#!/usr/bin/env bash

# Extracts the latest adopted slot from an Amaru log.
adopted_slot_from_log() {
  local log="$1"
  [[ -f "$log" ]] || { echo ""; return; }
  local slot
  slot="$(
    tail -n "${AMARU_SYNC_LOG_TAIL_LINES:-20000}" "$log" 2>/dev/null \
      | LC_ALL=C awk '
          /adopted tip/ {
            if (match($0, /"tip\.slot":"?[0-9]+"?/)) {
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
        /adopted tip/ {
          if (match($0, /"tip\.slot":"?[0-9]+"?/)) {
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
      ' "$log" 2>/dev/null
    )"
  fi
  echo "$slot"
}

# Reads the latest adopted slot from the middle Amaru log.
middle_amaru_adopted_slot() {
  adopted_slot_from_log "${AMARU_MIDDLE_LOG_FILE:-$LOGDIR/amaru-middle.log}"
}

# Reads the latest adopted slot from the downstream Amaru log.
downstream_adopted_slot() {
  adopted_slot_from_log "${AMARU_DOWNSTREAM_LOG_FILE:-$LOGDIR/amaru-downstream.log}"
}

amaru_network_from_log() {
  local log="$1"
  [[ -f "$log" ]] || { echo ""; return; }
  LC_ALL=C awk '
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
  ' "$log" 2>/dev/null
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

# Computes the absolute slot distance between two tips.
sync_gap() {
  local tip="$1"
  local adopted="$2"
  local gap=$(( tip - adopted ))
  if (( gap < 0 )); then
    gap=$(( -gap ))
  fi
  echo "$gap"
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

# Waits until both Amaru nodes are within the configured slot tolerance.
wait_for_amaru_sync_to_cardano_node() {
  local tolerance="$TX_SYNC_SLOT_TOLERANCE"
  local timeout="$TX_SYNC_TIMEOUT_SECONDS"
  local magic start now tip middle downstream middle_gap downstream_gap elapsed prev_middle prev_down prev_t poll_interval eta
  magic="$(network_magic)"
  wait_for_cardano_socket
  validate_amaru_runtime_network "${AMARU_MIDDLE_LOG_FILE:-$LOGDIR/amaru-middle.log}" "middle"
  validate_amaru_runtime_network "${AMARU_DOWNSTREAM_LOG_FILE:-$LOGDIR/amaru-downstream.log}" "downstream"
  poll_interval="$(sync_poll_interval_seconds)"
  start="$(date +%s)"
  prev_middle=""
  prev_down=""
  prev_t=""
  echo "[submit-tx] waiting for middle and downstream Amaru to catch up to cardano-node (tolerance: ${tolerance} slots, timeout: ${timeout}s)..."
  echo "[submit-tx] note: Amaru replays the ledger from its bootstrap snapshot, which can take ~1-2 hours to reach network tip"
  while true; do
    now="$(date +%s)"
    elapsed=$(( now - start ))
    tip="$(cardano_node_tip_slot "$magic" || true)"
    middle="$(middle_amaru_adopted_slot)"
    downstream="$(downstream_adopted_slot)"
    if [[ -n "$tip" && -n "$middle" && -n "$downstream" ]]; then
      middle_gap="$(sync_gap "$tip" "$middle")"
      downstream_gap="$(sync_gap "$tip" "$downstream")"
      eta=""
      if [[ -n "$prev_down" && -n "$prev_t" && "$now" -gt "$prev_t" ]]; then
        local d_slots d_secs max_gap
        d_slots=$(( (middle - prev_middle + downstream - prev_down) / 2 ))
        d_secs=$(( now - prev_t ))
        max_gap="$middle_gap"
        if (( downstream_gap > max_gap )); then
          max_gap="$downstream_gap"
        fi
        eta="$(eta_hint "$max_gap" "$d_slots" "$d_secs" "$poll_interval")"
      fi
      printf '[submit-tx] elapsed=%ss cardano=%s middle=%s middle_gap=%s downstream=%s downstream_gap=%s%s\n' "$elapsed" "$tip" "$middle" "$middle_gap" "$downstream" "$downstream_gap" "$eta"
      if (( middle_gap <= tolerance && downstream_gap <= tolerance )); then
        echo "[submit-tx] middle and downstream Amaru are synchronized with cardano-node; proceeding"
        return 0
      fi
      prev_middle="$middle"
      prev_down="$downstream"
      prev_t="$now"
    else
      echo "[submit-tx] elapsed=${elapsed}s cardano=${tip:-?} middle=${middle:-?} downstream=${downstream:-?} (still initializing)"
    fi
    if (( elapsed > timeout )); then
      die "Amaru nodes did not catch up to cardano-node within ${timeout}s (last middle gap: ${middle_gap:-unknown}, downstream gap: ${downstream_gap:-unknown})"
    fi
    sleep "$poll_interval"
  done
}

wait_for_downstream_slot() {
  local target_slot="$1"
  local timeout="$TX_SYNC_TIMEOUT_SECONDS"
  local start now elapsed downstream remaining prev_down prev_t poll_interval eta
  validate_amaru_runtime_network "${AMARU_DOWNSTREAM_LOG_FILE:-$LOGDIR/amaru-downstream.log}" "downstream"
  poll_interval="$(sync_poll_interval_seconds)"
  start="$(date +%s)"
  prev_down=""
  prev_t=""
  echo "[submit-tx] waiting for downstream Amaru to reach selected input availability slot ${target_slot} (timeout: ${timeout}s)..."
  while true; do
    now="$(date +%s)"
    elapsed=$(( now - start ))
    downstream="$(downstream_adopted_slot)"
    if [[ -n "$downstream" ]]; then
      remaining=$(( target_slot - downstream ))
      eta=""
      if (( remaining <= 0 )); then
        echo "[submit-tx] downstream Amaru reached slot ${downstream}; selected input UTxO should be available"
        return 0
      fi
      if [[ -n "$prev_down" && -n "$prev_t" && "$now" -gt "$prev_t" ]]; then
        local d_slots d_secs
        d_slots=$(( downstream - prev_down ))
        d_secs=$(( now - prev_t ))
        eta="$(eta_hint "$remaining" "$d_slots" "$d_secs" "$poll_interval")"
      fi
      printf '[submit-tx] elapsed=%ss target=%s downstream=%s remaining=%s%s\n' "$elapsed" "$target_slot" "$downstream" "$remaining" "$eta"
      prev_down="$downstream"
      prev_t="$now"
    else
      echo "[submit-tx] elapsed=${elapsed}s target=${target_slot} downstream=? (still initializing)"
    fi
    if (( elapsed > timeout )); then
      die "downstream Amaru did not reach selected input availability slot ${target_slot} within ${timeout}s (last downstream slot: ${downstream:-unknown})"
    fi
    sleep "$poll_interval"
  done
}
