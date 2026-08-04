#!/usr/bin/env bash

# Creates the demo log and run directories configured by the caller, plus the configuration
# directory process-compose looks for (it logs a warning on every invocation when it is absent,
# which is the case in a container where HOME points at a fresh volume).
ensure_dirs() {
  mkdir -p "$LOGDIR" "$RUNDIR" "${XDG_CONFIG_HOME:-$HOME/.config}/process-compose"
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

# Tools every demo needs, expected to come from the demo flox environment
# (scripts/demos/.flox) rather than whatever the host happens to have on PATH.
# Deliberately not listed: cardano-cli and cardano-node (installed by setup from a pinned,
# checksum-verified release), docker (host-provided, telemetry stack only), cargo and rustc
# (rustup shims manage their own toolchains), codesign/open/xdg-open (host-specific, optional).
DEMO_REQUIRED_TOOLS=(process-compose jq curl rsync awk rg tar fd gzip xxd
  sha256sum cmp mktemp tee sort tr head tail install date ps rg dirname basename)

# Verifies the demo runs inside the flox environment and that every required tool is
# available, warning when a tool resolves outside the environment (a sign of a broken
# activation). Set AMARU_DEMO_SKIP_TOOL_CHECK=true to bypass, at your own risk.
require_demo_tools() {
  truthy "${AMARU_DEMO_SKIP_TOOL_CHECK:-false}" && return 0
  [[ -n "${FLOX_ENV:-}" ]] ||
    die "the demos must run inside their flox environment: run 'flox activate' in scripts/demos (or set AMARU_DEMO_SKIP_TOOL_CHECK=true)"
  local tool resolved missing=()
  for tool in "${DEMO_REQUIRED_TOOLS[@]}"; do
    if ! resolved="$(command -v "$tool" 2>/dev/null)"; then
      missing+=("$tool")
    elif [[ "$resolved" != "$FLOX_ENV"/* ]]; then
      echo "warning: $tool resolves outside the flox environment: $resolved" >&2
    fi
  done
  [[ ${#missing[@]} -eq 0 ]] ||
    die "required tools missing from the flox environment: ${missing[*]}; run 'flox activate' in scripts/demos"
}

# Refuses to run a process-compose one-shot process as a scaled replica.
require_unscaled_process() {
  local process_name="$1" replica_num="${PC_REPLICA_NUM:-0}"
  if [[ ! "$replica_num" =~ ^0+$ ]]; then
    die "$process_name cannot be scaled: replica $replica_num would mutate shared demo directories"
  fi
}
