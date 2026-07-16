#!/usr/bin/env bash

# Creates the demo log and run directories configured by the caller.
ensure_dirs() {
  mkdir -p "$LOGDIR" "$RUNDIR"
}

# Prints an error message and exits the current script.
die() { echo "error: $*" >&2; exit 1; }

# Returns whether a command is available on PATH.
have() { command -v "$1" >/dev/null 2>&1; }

truthy() {
  case "${1:-}" in
    1 | true | TRUE | yes | YES | on | ON) return 0 ;;
    *) return 1 ;;
  esac
}

# Refuses to run a process-compose one-shot process as a scaled replica.
require_unscaled_process() {
  local process_name="$1" replica_num="${PC_REPLICA_NUM:-0}"
  if [[ ! "$replica_num" =~ ^0+$ ]]; then
    die "$process_name cannot be scaled: replica $replica_num would mutate shared demo directories"
  fi
}
