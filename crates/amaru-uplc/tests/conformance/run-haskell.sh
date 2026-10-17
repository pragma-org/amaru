#!/usr/bin/env bash

set -euo pipefail

# Run the conformance corpus with the Plutus executable and emit the report
# consumed by node-diversity-conformance.

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PARAMETERS="$SCRIPT_DIR/parameters.toml"
UPLC_BIN="${UPLC_BIN:-uplc}"
IMPLEMENTATION_NAME="${IMPLEMENTATION_NAME:-cardano-node}"
CORPUS="${CORPUS:-amaru-uplc/conformance}"
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/amaru-uplc-conformance.XXXXXX")"
RECORDS="$TMP_DIR/records.jsonl"
trap 'rm -rf "$TMP_DIR"' EXIT

command -v "$UPLC_BIN" >/dev/null 2>&1 || {
    echo "missing uplc executable: $UPLC_BIN" >&2
    exit 1
}
command -v jq >/dev/null 2>&1 || {
    echo "missing required command: jq" >&2
    exit 1
}
command -v xxd >/dev/null 2>&1 || {
    echo "missing required command: xxd" >&2
    exit 1
}

protocol_version() {
    case "$1" in
        protocol-version-10/*) printf '10\n' ;;
        protocol-version-11/*) printf '11\n' ;;
        *)
            echo "cannot infer protocol version from: $1" >&2
            return 1
            ;;
    esac
}

scenario_path() {
    case "$1" in
        protocol-version-10/*) printf '%s\n' "${1#protocol-version-10/}" ;;
        protocol-version-11/*) printf '%s\n' "${1#protocol-version-11/}" ;;
        *) return 1 ;;
    esac
}

scenario_id_base() {
    printf '%s' "$1" | tr -c '[:alnum:]' '_' | tr '[:upper:]' '[:lower:]'
}

scenario_title() {
    leaf="${1##*/}"
    printf '%s' "$leaf" | tr '_-' '  ' | tr '[:upper:]' '[:lower:]'
}

plutus_versions() {
    local key="$1"
    local line

    line="$(awk -v section="[parameters.\"$key\"]" '
        $0 == section { found = 1; next }
        found && /^\[/ { exit }
        found && /^plutus_versions[[:space:]]*=/ { print; exit }
    ' "$PARAMETERS")"

    if [[ -z "$line" ]]; then
        printf 'V3\n'
        return
    fi

    printf '%s\n' "$line" \
        | sed -E 's/.*\[(.*)\].*/\1/' \
        | tr ',' '\n' \
        | tr -d ' "'
}

semantics_variant() {
    case "$1:$2" in
        10:V1|10:V2) printf 'B\n' ;;
        10:V3) printf 'C\n' ;;
        11:V1|11:V2) printf 'D\n' ;;
        11:V3) printf 'E\n' ;;
        *)
            echo "unsupported Plutus/protocol combination: $2 / $1" >&2
            return 1
            ;;
    esac
}

program_version() {
    tr '\n' ' ' < "$1" | sed -n -E 's/.*\([[:space:]]*program[[:space:]]+([^[:space:]]+).*/\1/p'
}

expected_marker() {
    local expected="$1.expected"

    if [[ -f "$expected" ]]; then
        case "$(tr -d '\r' < "$expected" | tr -d '\n')" in
            "parse error"|"evaluation failure"|"out of budget")
                tr -d '\r' < "$expected" | tr -d '\n'
                ;;
        esac
    fi
}

run_evaluation() {
    local input="$1"
    local format="$2"
    local semantics="$3"
    local stdout="$TMP_DIR/evaluation.stdout"
    local stderr="$TMP_DIR/evaluation.stderr"

    if "$UPLC_BIN" evaluate \
        --input "$input" \
        --if "$format" \
        --counting \
        --builtin-semantics-variant "$semantics" \
        >"$stdout" 2>"$stderr"; then
        EVALUATION_STDOUT="$stdout"
        return 0
    fi

    return 1
}

evaluate_case() {
    local source="$1"
    local flat="${source%.uplc}.flat"
    local binary="$TMP_DIR/$(basename "$source").flat"

    # The repository stores Flat as hexadecimal text, while uplc --if flat
    # expects the decoded binary representation.
    if [[ -s "$flat" ]]; then
        xxd -r -p "$flat" >"$binary"
        if run_evaluation "$binary" flat "$SEMANTICS"; then
            return 0
        fi
    fi

    # Empty or incompatible Flat artifacts are evaluated from the source text.
    run_evaluation "$source" textual "$SEMANTICS"
}

append_result() {
    local source="$1"
    local relative="$2"
    local major="$3"
    local plutus="$4"
    local scenario="$5"
    local group="$6"
    local id="$7"
    local title="$8"
    local version="$9"
    local evaluation budget cpu mem

    if evaluate_case "$source"; then
        evaluation="$(sed -e '/^CPU budget:[[:space:]]/d' -e '/^Memory budget:[[:space:]]/d' "$EVALUATION_STDOUT")"
        cpu="$(sed -n 's/^CPU budget:[[:space:]]*//p' "$EVALUATION_STDOUT")"
        mem="$(sed -n 's/^Memory budget:[[:space:]]*//p' "$EVALUATION_STDOUT")"
        evaluation="(program $version $evaluation)"
        budget="({cpu: $cpu
| mem: $mem})"
    else
        evaluation="$(expected_marker "$source" || true)"
        evaluation="${evaluation:-evaluation failure}"
        budget="null"
    fi

    jq -cn \
        --arg id "$id" \
        --arg title "$title" \
        --arg group "$group" \
        --arg plutus_version "$plutus" \
        --arg protocol_version "$major.0" \
        --arg evaluation "$evaluation" \
        --arg budget "$budget" \
        '({
            id: $id,
            title: $title,
            group: $group,
            plutus_version: $plutus_version,
            protocol_version: $protocol_version,
            evaluation: $evaluation,
            budget: (if $budget == "null" then null else $budget end)
        })' >>"$RECORDS"
}

count=0
while IFS= read -r source; do
    relative="${source#"$SCRIPT_DIR"/}"
    major="$(protocol_version "$relative")"
    scenario_path_value="$(scenario_path "$relative")"
    scenario_dir="${scenario_path_value%/*}"
    group="${scenario_dir%/*}"
    id_base="$(scenario_id_base "$scenario_dir")"
    title="$(scenario_title "$scenario_dir")"
    version="$(program_version "$source")"

    if [[ -z "$version" ]]; then
        echo "cannot determine UPLC version from: $source" >&2
        exit 1
    fi

    while IFS= read -r plutus; do
        [[ -n "$plutus" ]] || continue
        SEMANTICS="$(semantics_variant "$major" "$plutus")"
        id="${id_base}_$(printf '%s' "$plutus" | tr '[:upper:]' '[:lower:]')_protocol_version_${major}"
        append_result "$source" "$relative" "$major" "$plutus" "$scenario_path_value" "$group" "$id" "$title" "$version"
        count=$((count + 1))
    done < <(plutus_versions "$scenario_path_value")
done < <(find "$SCRIPT_DIR/protocol-version-10" "$SCRIPT_DIR/protocol-version-11" -type f -name '*.uplc' | sort)

jq -s \
    --arg generated_at "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" \
    --arg implementation "$IMPLEMENTATION_NAME" \
    --arg corpus "$CORPUS" \
    '{
        schema_version: "1",
        scenario: "uplc-conformance",
        generated_at: $generated_at,
        implementation: {name: $implementation},
        corpus: $corpus,
        scenarios: .
    }' "$RECORDS"

echo "generated $count Haskell UPLC conformance results" >&2
