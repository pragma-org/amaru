#!/usr/bin/env bash

# Orchestrates a demo's process-compose lifecycle: validating the configuration before
# startup, generating the per-network process file, and the up/down/status commands.
#
# Callers must set SCRIPT_DIR (the demo directory containing process-compose.yaml) and
# PUBLIC_UPSTREAM_EXCLUDED_PROCESSES (the local-only processes removed from the generated
# file in public-upstream mode), and define a validate_config function with the
# demo-specific configuration checks.

require_runtime_processes_stopped() {
  local process_name="$1" processes cardano_processes amaru_processes
  processes="$(ps -Ao command=)"
  cardano_processes="$(rg -F -- "cardano-node" <<<"$processes" || true)"
  amaru_processes="$(rg -F -- "$(amaru_node_binary)" <<<"$processes" || true)"
  if { [[ -n "$cardano_processes" ]] && rg -q -F -- "--socket-path $(cardano_node_socket_file)" <<<"$cardano_processes"; } ||
    { [[ -n "$amaru_processes" ]] && rg -q -F -- "$RUNDIR" <<<"$amaru_processes"; }; then
    die "$process_name cannot run while demo runtime processes are active; run ./process-compose.sh down first"
  fi
}

validate_up() {
  require_demo_tools
  if declare -F validate_startup_config >/dev/null; then
    validate_startup_config
  else
    validate_config
  fi
  case "$BOOTSTRAP_AMARU_DATABASES" in
    auto | true | TRUE | 1 | yes | YES | on | ON) ;;
    false | FALSE | 0 | no | NO | off | OFF) validate_amaru_source_databases ;;
    *) die "BOOTSTRAP_AMARU_DATABASES must be auto, true, or false" ;;
  esac
}

# Generates the per-network process-compose file; in public-upstream mode the processes
# listed in PUBLIC_UPSTREAM_EXCLUDED_PROCESSES, and dependencies on them, are removed.
process_compose_file() {
  local generated network_label public_upstream excluded pattern
  network_label="$(printf '%s' "$NETWORK" | tr '[:lower:]' '[:upper:]')"
  public_upstream=false
  if public_cardano_upstream_enabled; then
    public_upstream=true
  fi
  excluded="${PUBLIC_UPSTREAM_EXCLUDED_PROCESSES[*]:-}"
  generated="$RUNDIR/generated/process-compose.$NETWORK.yaml"
  mkdir -p "$(dirname "$generated")"
  awk -v network_label="$network_label" -v public_upstream="$public_upstream" -v excluded="$excluded" '
    BEGIN {
      count = split(excluded, excluded_list, " ")
      for (i = 1; i <= count; i++) excluded_set[excluded_list[i]] = 1
    }
    function key_of(line,    key) {
      key = line
      sub(/^ +/, "", key)
      sub(/:.*$/, "", key)
      return key
    }
    /^version:/ {
      print
      print "name: \"" network_label "\""
      next
    }
    public_upstream == "true" && /^  [^ ]+:/ && key_of($0) in excluded_set { skip_process = 1; next }
    skip_process && /^  [^ ]/ { skip_process = 0 }
    skip_process { next }
    public_upstream == "true" && /^      [^ ]+:/ && key_of($0) in excluded_set { skip_dependency = 1; next }
    skip_dependency && /^        / { next }
    { skip_dependency = 0; print }
  ' "$SCRIPT_DIR/process-compose.yaml" >"$generated"
  pattern="${excluded// /|}"
  if public_cardano_upstream_enabled && [[ -n "$pattern" ]] && rg -q "^ *($pattern):" "$generated"; then
    die "generated $generated still references local-only processes; update PUBLIC_UPSTREAM_EXCLUDED_PROCESSES or process-compose.yaml"
  fi
  echo "$generated"
}

up() {
  have process-compose || die "process-compose not found"
  validate_up
  local compose_file
  local pc_args=()
  compose_file="$(process_compose_file)"
  # Disable the interactive TUI when running headless (e.g. inside a container) so the
  # process logs stream to stdout instead.
  if [[ ! -t 0 || ! -t 1 ]]; then
    pc_args+=(--tui=false)
  fi
  cd "$SCRIPT_DIR" || return
  exec process-compose -f "$compose_file" up --ordered-shutdown ${pc_args[@]+"${pc_args[@]}"}
}

down() {
  have process-compose || die "process-compose not found"
  cd "$SCRIPT_DIR" || return
  process-compose down --ordered-shutdown
}

status() {
  have process-compose || die "process-compose not found"
  cd "$SCRIPT_DIR" || return
  process-compose list
}
