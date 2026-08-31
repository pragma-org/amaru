#!/usr/bin/env bash

# Removes ANSI escape sequences from stdin. The nodes colorize their output so the log levels
# stand out in the process-compose panes, and that output is also what lands in the log files,
# so anything parsing those files has to see through the escapes.
strip_ansi() {
  LC_ALL=C awk '{ gsub(/\033\[[0-9;]*[A-Za-z]/, ""); print }'
}

# Extracts the latest adopted slot from an Amaru log.
adopted_slot_from_log() {
  local log="$1"
  [[ -f "$log" ]] || { echo ""; return; }
  local slot
  slot="$(
    tail -n 20000 "$log" 2>/dev/null \
      | strip_ansi \
      | LC_ALL=C awk '
          /tip\.adopt|adopted tip/ {
            if (match($0, / slot=[0-9]+/)) {
              slot = substr($0, RSTART + 6, RLENGTH - 6)
            } else if (match($0, /"slot":"?[0-9]+"?/)) {
              slot = substr($0, RSTART, RLENGTH)
              sub(/^"slot":"?/, "", slot)
              sub(/"?$/, "", slot)
            } else if (match($0, /"tip\.slot":"?[0-9]+"?/)) {
              slot = substr($0, RSTART, RLENGTH)
              sub(/^"tip\.slot":"?/, "", slot)
              sub(/"?$/, "", slot)
            } else if (match($0, /tip\.slot=[0-9]+/)) {
              slot = substr($0, RSTART + 9, RLENGTH - 9)
            }
          }
          slot == "" && /build_ledger/ {
            if (match($0, /tip\.slot=[0-9]+/)) {
              initial_slot = substr($0, RSTART + 9, RLENGTH - 9)
            }
          }
          END { if (slot != "") print slot; else if (initial_slot != "") print initial_slot }
        '
  )"
  if [[ -z "$slot" ]]; then
    slot="$(
      LC_ALL=C awk '
        /tip\.adopt|adopted tip/ {
          if (match($0, / slot=[0-9]+/)) {
            slot = substr($0, RSTART + 6, RLENGTH - 6)
          } else if (match($0, /"slot":"?[0-9]+"?/)) {
            slot = substr($0, RSTART, RLENGTH)
            sub(/^"slot":"?/, "", slot)
            sub(/"?$/, "", slot)
          } else if (match($0, /"tip\.slot":"?[0-9]+"?/)) {
            slot = substr($0, RSTART, RLENGTH)
            sub(/^"tip\.slot":"?/, "", slot)
            sub(/"?$/, "", slot)
          } else if (match($0, /tip\.slot=[0-9]+/)) {
            slot = substr($0, RSTART + 9, RLENGTH - 9)
          }
        }
        slot == "" && /build_ledger/ {
          if (match($0, /tip\.slot=[0-9]+/)) {
            initial_slot = substr($0, RSTART + 9, RLENGTH - 9)
          }
        }
        END { if (slot != "") print slot; else if (initial_slot != "") print initial_slot }
      ' < <(strip_ansi <"$log" 2>/dev/null)
    )"
  fi
  echo "$slot"
}

# Reads the latest adopted slot from the downstream Amaru log.
downstream_adopted_slot() {
  adopted_slot_from_log "${AMARU_DOWNSTREAM_LOG_FILE:-$LOGDIR/amaru-downstream.log}"
}

amaru_network_from_log() {
  local log="$1"
  [[ -f "$log" ]] || { echo ""; return; }
  strip_ansi <"$log" 2>/dev/null | LC_ALL=C awk '
    {
      if (match($0, /"network":"[^"]+"/)) {
        network = substr($0, RSTART, RLENGTH)
        sub(/^"network":"/, "", network)
        sub(/"$/, "", network)
      } else if (match($0, /network="?[A-Za-z0-9_-]+"?/)) {
        network = substr($0, RSTART, RLENGTH)
        sub(/^network="?/, "", network)
        sub(/"?$/, "", network)
      }
    }
    END { if (network != "") print network }
  '
}

validate_amaru_runtime_network() {
  local log="$1" label="$2" expected="${NETWORK:-}" actual
  [[ -n "$expected" ]] || return 0
  actual="$(amaru_network_from_log "$log")"
  [[ -n "$actual" ]] || return 0
  if [[ "$actual" != "$expected" ]]; then
    die "$label Amaru is running on network=$actual, but this command is configured with AMARU_NETWORK=$expected"
  fi
}

sync_poll_interval_seconds() {
  echo "${TX_SYNC_POLL_INTERVAL_SECONDS:-15}"
}

eta_hint() {
  local remaining="$1" observed_slots="$2" observed_seconds="$3" poll_interval="$4"
  local rate eta
  if (( observed_slots <= observed_seconds )); then
    return 0
  fi
  rate=$(( observed_slots - observed_seconds ))
  eta=$(( (remaining * observed_seconds + rate - 1) / rate ))
  if (( eta < poll_interval )); then
    printf ' eta<%ss' "$poll_interval"
  else
    printf ' eta=%ss' "$eta"
  fi
}

wait_for_downstream_slot() {
  wait_for_amaru_slot \
    "${AMARU_DOWNSTREAM_LOG_FILE:-$LOGDIR/amaru-downstream.log}" \
    "downstream" \
    "$1" \
    "$TX_SYNC_TIMEOUT_SECONDS"
}

# Waits until an Amaru process has adopted a target slot according to its structured log.
wait_for_amaru_slot() {
  local log="$1" label="$2" target_slot="$3" timeout="${4:-$TX_SYNC_TIMEOUT_SECONDS}"
  local start now elapsed adopted remaining prev_adopted prev_t poll_interval eta
  validate_amaru_runtime_network "$log" "$label"
  poll_interval="$(sync_poll_interval_seconds)"
  start="$(date +%s)"
  prev_adopted=""
  prev_t=""
  echo "[submit-tx] waiting for $label Amaru to reach selected input availability slot ${target_slot} (timeout: ${timeout}s)..."
  while true; do
    now="$(date +%s)"
    elapsed=$(( now - start ))
    adopted="$(adopted_slot_from_log "$log")"
    if [[ -n "$adopted" ]]; then
      remaining=$(( target_slot - adopted ))
      eta=""
      if (( remaining <= 0 )); then
        echo "[submit-tx] $label Amaru reached slot ${adopted}; selected input UTxO should be available"
        return 0
      fi
      if [[ -n "$prev_adopted" && -n "$prev_t" && "$now" -gt "$prev_t" ]]; then
        local d_slots d_secs
        d_slots=$(( adopted - prev_adopted ))
        d_secs=$(( now - prev_t ))
        eta="$(eta_hint "$remaining" "$d_slots" "$d_secs" "$poll_interval")"
      fi
      printf '[submit-tx] elapsed=%ss target=%s %s=%s remaining=%s%s\n' "$elapsed" "$target_slot" "$label" "$adopted" "$remaining" "$eta"
      prev_adopted="$adopted"
      prev_t="$now"
    else
      echo "[submit-tx] elapsed=${elapsed}s target=${target_slot} ${label}=? (still initializing)"
    fi
    if (( elapsed > timeout )); then
      die "$label Amaru did not reach selected input availability slot ${target_slot} within ${timeout}s (last adopted slot: ${adopted:-unknown})"
    fi
    sleep "$poll_interval"
  done
}
