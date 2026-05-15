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
