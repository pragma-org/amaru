#!/usr/bin/env bash

set -euo pipefail

UPLC_BIN="${UPLC_BIN:-uplc}"
JOBS="${JOBS:-1}"
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
SAMPLES_DIR="${SAMPLES_DIR:-$SCRIPT_DIR/samples}"

if [[ $# -ne 0 ]]; then
    echo "run-uplc.sh is configured with UPLC_BIN, JOBS, and SAMPLES_DIR" >&2
    exit 2
fi

command -v jq >/dev/null 2>&1 || {
    echo "missing required command: jq" >&2
    exit 1
}
command -v "$UPLC_BIN" >/dev/null 2>&1 || {
    echo "missing UPLC executable: $UPLC_BIN" >&2
    exit 1
}
[[ -d "$SAMPLES_DIR" ]] || {
    echo "missing samples directory: $SAMPLES_DIR" >&2
    exit 1
}
[[ "$JOBS" =~ ^[1-9][0-9]*$ ]] || {
    echo "JOBS must be a positive integer: $JOBS" >&2
    exit 1
}

RESULTS_DIR="$(mktemp -d "${TMPDIR:-/tmp}/run-uplc.XXXXXX")"
RAW_DIR="$RESULTS_DIR/raw"
STREAM="$RESULTS_DIR/report.jsonl"
METADATA="$RESULTS_DIR/metadata.json"
AGGREGATE="$SAMPLES_DIR/../cardano-node.json"
mkdir -p "$RAW_DIR"

trap 'rm -rf "$RESULTS_DIR"' EXIT

sample_id() {
    basename -- "$1"
}

protocol_version_for_epoch() {
    local epoch="$1"

    if (( epoch >= 644 )); then
        echo 11
    elif (( epoch >= 537 )); then
        echo 10
    elif (( epoch >= 507 )); then
        echo 9
    else
        echo 8
    fi
}

append_result() {
    local result="$1"

    jq -e -c '
        if (.scenario == "uplc-turbo") and
           ((.benchmarks | type) == "array") and
           ((.benchmarks | length) == 1) and
           ((.benchmarks[0] | keys | sort) == [
             "bench", "budget_cpu", "budget_mem", "cpu_us", "mem_bytes",
             "plutus_version", "protocol_version"
           ]) and
           ((.benchmarks[0].bench | type) == "string") and
           ((.benchmarks[0].cpu_us | type) == "number") and
           ((.benchmarks[0].mem_bytes == null) or ((.benchmarks[0].mem_bytes | type) == "number"))
        then .benchmarks[0]
        else error("invalid UPLC benchmark report")
        end
    ' "$result" >>"$STREAM"

    if [[ ! -s "$METADATA" ]]; then
        jq 'del(.benchmarks, .protocol_version, .partial)' "$result" >"$METADATA"
    fi
}

run_sample() {
    local sample="$1"
    local id epoch protocol_version raw temporary log

    id="$(sample_id "$sample")"
    epoch="$(basename -- "$(dirname -- "$sample")")"
    if [[ ! "$epoch" =~ ^[0-9]+$ ]]; then
        echo "cannot infer epoch from sample parent directory: $sample" >&2
        return 1
    fi
    epoch=$((10#$epoch))
    protocol_version="$(protocol_version_for_epoch "$epoch")"
    raw="$RAW_DIR/$id.json"
    temporary="$(mktemp "$RAW_DIR/.${id}.XXXXXX")"
    log="$RAW_DIR/$id.stderr.log"

    if ! "$UPLC_BIN" benchmark \
        --input "$sample" \
        --if serialised \
        --major-protocol-version "$protocol_version" \
        --json >"$temporary" 2>"$log"; then
        cat "$log" >&2
        echo "benchmark failed for $sample" >&2
        return 1
    fi

    mv -f "$temporary" "$raw"
}

flush_batch() {
    local failed=0
    local index sample id

    [[ "${#pids[@]}" -gt 0 ]] || return 0

    for index in "${!pids[@]}"; do
        if ! wait "${pids[$index]}"; then
            failed=1
        fi
    done
    [[ "$failed" -eq 0 ]] || return 1

    for sample in "${batch_samples[@]}"; do
        id="$(sample_id "$sample")"
        append_result "$RAW_DIR/$id.json"
    done

    pids=()
    batch_samples=()
}

samples=()
while IFS= read -r sample; do
    samples+=("$sample")
done < <(find "$SAMPLES_DIR" -type f -name '*.flat' | sort)
total="${#samples[@]}"
[[ "$total" -gt 0 ]] || {
    echo "no .flat samples found in $SAMPLES_DIR" >&2
    exit 1
}

declare -a pids=()
declare -a batch_samples=()
scheduled=0

for sample in "${samples[@]}"; do
    scheduled=$((scheduled + 1))
    printf '[%s/%s] %s\n' "$scheduled" "$total" "$sample"
    run_sample "$sample" &
    pids+=("$!")
    batch_samples+=("$sample")

    if [[ "${#pids[@]}" -ge "$JOBS" ]]; then
        flush_batch
    fi
done

flush_batch

temporary="$(mktemp "$RESULTS_DIR/.aggregate.XXXXXX")"
jq -s --slurpfile metadata "$METADATA" \
    '$metadata[0] + {benchmarks: .}' \
    "$STREAM" >"$temporary"
mv -f "$temporary" "$AGGREGATE"

count="$(wc -l <"$STREAM" | tr -d ' ')"
printf 'aggregate: %s (%s benchmarks)\n' "$AGGREGATE" "$count"
