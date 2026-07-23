#!/usr/bin/env bash

# Builds and runs Amaru nodes for the demos.
#
# Each node is configured through AMARU_<NAME>_* variables (LOG_FILE, DATA_DIR, LOG, TRACE,
# WITH_OPEN_TELEMETRY, WITH_JSON_TRACES, OTEL_SERVICE_NAME, OTEL_SERVICE_INSTANCE_ID), where
# <NAME> is the uppercase node name passed to run_amaru_node. The shared OTEL_* exporter
# variables, AMARU_TRACE_EMIT_PRIVATE, AMARU_DIR, NETWORK, and BUILD_PROFILE are read from
# the environment prepared by the demo script.

target_profile_dir() {
  case "$BUILD_PROFILE" in
    dev) echo "debug" ;;
    release) echo "release" ;;
    *) echo "$BUILD_PROFILE" ;;
  esac
}

amaru_node_binary() {
  echo "${CARGO_TARGET_DIR:-$AMARU_DIR/target}/$(target_profile_dir)/amaru"
}

build_amaru_node_binary() {
  cd "$AMARU_DIR" || return
  echo "[initialize] building Amaru node binary with BUILD_PROFILE=$BUILD_PROFILE..."
  AMARU_NETWORK="$NETWORK" cargo build --profile "$BUILD_PROFILE" --bin amaru
}

require_amaru_node_binary() {
  [[ -x "$(amaru_node_binary)" ]] || die "Amaru node binary not found: $(amaru_node_binary); run ./process-compose.sh initialize first"
}

raise_open_files_limit() {
  ulimit -n 65536 2>/dev/null ||
    echo "warning: could not raise the open file limit to 65536; using $(ulimit -n)" >&2
}

# Resolves one AMARU_<name>_<suffix> per-node configuration variable.
amaru_node_var() {
  local name="$1" suffix="$2"
  local var="AMARU_${name}_${suffix}"
  [[ -n "${!var:-}" ]] || die "$var must be set to run the $name Amaru node"
  echo "${!var}"
}

# Runs one Amaru node: run_amaru_node <NAME> <peer-address> <listen-address> [extra args...].
run_amaru_node() {
  local name="$1" peer_address="$2" listen_address="$3"
  shift 3
  local log_file data_dir with_otel log_filter trace_filter otel_service_name otel_instance_id trace_arg=""
  log_file="$(amaru_node_var "$name" LOG_FILE)"
  data_dir="$(amaru_node_var "$name" DATA_DIR)"
  with_otel="$(amaru_node_var "$name" WITH_OPEN_TELEMETRY)"
  log_filter="$(amaru_node_var "$name" LOG)"
  trace_filter="$(amaru_node_var "$name" TRACE)"
  otel_service_name="$(amaru_node_var "$name" OTEL_SERVICE_NAME)"
  otel_instance_id="$(amaru_node_var "$name" OTEL_SERVICE_INSTANCE_ID)"
  if truthy "$(amaru_node_var "$name" WITH_JSON_TRACES)"; then
    trace_arg="--with-json-traces"
  fi

  cd "$AMARU_DIR" || return
  mkdir -p "$(dirname "$log_file")"
  : >"$log_file"
  validate_network_config
  export AMARU_WITH_OPEN_TELEMETRY="$with_otel"
  export AMARU_LOG="$log_filter"
  export AMARU_TRACE="$trace_filter"
  export AMARU_TRACE_EMIT_PRIVATE
  export OTEL_SERVICE_NAME="$otel_service_name"
  export OTEL_SERVICE_INSTANCE_ID="$otel_instance_id"
  export OTEL_EXPORTER_OTLP_ENDPOINT
  export OTEL_EXPORTER_OTLP_METRICS_ENDPOINT
  export OTEL_METRIC_EXPORT_INTERVAL=1000
  export OTEL_BSP_MAX_QUEUE_SIZE=65536
  export OTEL_BSP_MAX_EXPORT_BATCH_SIZE=256
  export OTEL_BSP_SCHEDULE_DELAY=500
  export OTEL_BSP_EXPORT_TIMEOUT=30000
  raise_open_files_limit
  require_amaru_node_binary
  mark_database_dir_dirty "$data_dir/chain.$NETWORK.db"
  mark_database_dir_dirty "$data_dir/ledger.$NETWORK.db"
  "$(amaru_node_binary)" ${trace_arg:+"$trace_arg"} run \
    --peer-address "$peer_address" \
    --listen-address "$listen_address" \
    --chain-dir "$data_dir/chain.$NETWORK.db" \
    --ledger-dir "$data_dir/ledger.$NETWORK.db" \
    "$@" \
    2>&1 | tee "$log_file"
}

# Readiness probe: the node's log contains its listening line.
ready_amaru_node_listening() {
  local log_file="$1"
  [[ -f "$log_file" ]] || exit 1
  grep -q listening "$log_file" 2>/dev/null
}

# Readiness probe: the node's HTTP submit API answers on the given address.
ready_amaru_submit_api() {
  curl -sS -o /dev/null --max-time 2 "http://$1/" >/dev/null 2>&1
}
