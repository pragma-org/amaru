#!/usr/bin/env bash

# Extracts the latest adopted slot from an Amaru JSON trace log.
adopted_slot_from_log() {
  local log="$1"
  [[ -f "$log" ]] || { echo ""; return; }
  local line
  line="$(
    tail -n "${AMARU_SYNC_LOG_TAIL_LINES:-20000}" "$log" 2>/dev/null \
      | LC_ALL=C awk '/"adopted tip"/ { line = $0 } END { if (line != "") print line }'
  )"
  if [[ -z "$line" ]]; then
    line="$(
      LC_ALL=C awk '/"adopted tip"/ { line = $0 } END { if (line != "") print line }' "$log" 2>/dev/null
    )"
  fi
  sed -nE 's/.*"tip\.slot":"([0-9]+)".*/\1/p' <<< "$line" || true
}

# Reads the latest adopted slot from the middle Amaru log.
middle_amaru_adopted_slot() {
  adopted_slot_from_log "${AMARU_MIDDLE_LOG_FILE:-$LOGDIR/amaru-middle.log}"
}

# Reads the latest adopted slot from the downstream Amaru log.
downstream_adopted_slot() {
  adopted_slot_from_log "${AMARU_DOWNSTREAM_LOG_FILE:-$LOGDIR/amaru-downstream.log}"
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

# Waits until both Amaru nodes are within the configured slot tolerance.
wait_for_amaru_sync_to_cardano_node() {
  local tolerance="$TX_SYNC_SLOT_TOLERANCE"
  local timeout="$TX_SYNC_TIMEOUT_SECONDS"
  local magic start now tip middle downstream middle_gap downstream_gap elapsed prev_middle prev_down prev_t rate eta
  magic="$(network_magic)"
  wait_for_cardano_socket
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
        if (( d_slots > d_secs )); then
          rate=$(( (d_slots - d_secs) ))
          if (( rate > 0 )); then
            eta=" eta=$(( max_gap * d_secs / rate ))s"
          fi
        fi
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
    sleep 15
  done
}

wait_for_downstream_slot() {
  local target_slot="$1"
  local timeout="$TX_SYNC_TIMEOUT_SECONDS"
  local start now elapsed downstream remaining prev_down prev_t rate eta
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
        if (( d_slots > d_secs )); then
          rate=$(( d_slots - d_secs ))
          if (( rate > 0 )); then
            eta=" eta=$(( remaining * d_secs / rate ))s"
          fi
        fi
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
    sleep 15
  done
}
