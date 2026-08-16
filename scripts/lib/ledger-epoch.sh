#!/usr/bin/env bash

is_epoch_number() {
  [[ "$1" =~ ^[0-9]+$ ]]
}

epoch_after() {
  local start_epoch="$1"
  local extra_epochs="$2"

  if ! is_epoch_number "$start_epoch" || ! is_epoch_number "$extra_epochs"; then
    echo "Error: start_epoch and extra_epochs must be non-negative integers." >&2
    return 1
  fi

  start_epoch=$((10#$start_epoch))
  extra_epochs=$((10#$extra_epochs))
  echo $((start_epoch + extra_epochs))
}

infer_ledger_start_epoch() {
  local ledger_dir="$1"
  local latest_snapshot_epoch=""
  local snapshot_dir
  local snapshot_epoch

  for snapshot_dir in "$ledger_dir"/*; do
    [ -d "$snapshot_dir" ] || continue
    snapshot_epoch="${snapshot_dir##*/}"
    is_epoch_number "$snapshot_epoch" || continue
    snapshot_epoch=$((10#$snapshot_epoch))

    if [ -z "$latest_snapshot_epoch" ] || (( snapshot_epoch > latest_snapshot_epoch )); then
      latest_snapshot_epoch="$snapshot_epoch"
    fi
  done

  if [ -z "$latest_snapshot_epoch" ]; then
    echo "Error: could not infer the starting epoch from '$ledger_dir'." >&2
    return 1
  fi

  epoch_after "$latest_snapshot_epoch" 1
}
