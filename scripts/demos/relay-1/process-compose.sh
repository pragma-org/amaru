#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEMO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
DEMO_COMMON_DIR="$DEMO_DIR/common"
AMARU_DIR="${AMARU_DIR:-$(cd "$SCRIPT_DIR/../../.." && pwd)}"

DEMO_NAME="relay-1"
LOGDIR="${LOGDIR:-/tmp/amaru-$DEMO_NAME}"
RUNDIR="${RUNDIR:-$SCRIPT_DIR/run}"
NETWORK="${AMARU_NETWORK:-preprod}"
BUILD_PROFILE="${BUILD_PROFILE:-dev}"
REFRESH_FROM_MITHRIL="${REFRESH_FROM_MITHRIL:-auto}"
CARDANO_NODE_INIT_FROM_MITHRIL="${CARDANO_NODE_INIT_FROM_MITHRIL:-auto}"
CARDANO_NODE_RELEASE_VERSION="${CARDANO_NODE_RELEASE_VERSION:-11.0.1}"
DEFAULT_CARDANO_NODE_HOME="$LOGDIR/cardano-node-$CARDANO_NODE_RELEASE_VERSION"
CARDANO_NODE_HOME_WAS_SET=false
if [[ -n "${CARDANO_NODE_HOME:-}" ]]; then
  CARDANO_NODE_HOME_WAS_SET=true
else
  CARDANO_NODE_HOME="$DEFAULT_CARDANO_NODE_HOME"
fi
MITHRIL_REFRESH_DIR="${MITHRIL_REFRESH_DIR:-$RUNDIR/mithril-refresh}"
MITHRIL_SNAPSHOTS_DIR="${AMARU_MITHRIL_SNAPSHOTS_DIR:-$AMARU_DIR/mithril-snapshots}"
MITHRIL_REFRESH_LOG_FILE="${MITHRIL_REFRESH_LOG_FILE:-$LOGDIR/mithril-refresh.log}"
AMARU_MIDDLE_LOG_FILE="${AMARU_MIDDLE_LOG_FILE:-$LOGDIR/amaru-middle.log}"
AMARU_DOWNSTREAM_LOG_FILE="${AMARU_DOWNSTREAM_LOG_FILE:-$LOGDIR/amaru-downstream.log}"
AMARU_MIDDLE_DATA_DIR="${AMARU_MIDDLE_DATA_DIR:-$RUNDIR/amaru}"
AMARU_DOWNSTREAM_DATA_DIR="${AMARU_DOWNSTREAM_DATA_DIR:-$RUNDIR/amaru-downstream}"
DEMO_LOG_FILES=(
  "$LOGDIR/cardano-upstream.log"
  "$LOGDIR/submit-tx.log"
  "$LOGDIR/prepare-wallet.log"
  "$AMARU_MIDDLE_LOG_FILE"
  "$AMARU_DOWNSTREAM_LOG_FILE"
)
AMARU_CHAIN_SOURCE_DIR="$MITHRIL_REFRESH_DIR/chain.$NETWORK.db"
AMARU_LEDGER_SOURCE_DIR="$MITHRIL_REFRESH_DIR/ledger.$NETWORK.db"

default_tx_payment_skey() {
  if [[ -f "$SCRIPT_DIR/run/$NETWORK-wallet/payment.skey" ]]; then
    echo "$SCRIPT_DIR/run/$NETWORK-wallet/payment.skey"
  elif [[ -f "$SCRIPT_DIR/keys/$NETWORK/payment.skey" ]]; then
    echo "$SCRIPT_DIR/keys/$NETWORK/payment.skey"
  else
    echo "$SCRIPT_DIR/keys/payment.skey"
  fi
}

TX_PAYMENT_SKEY="$(default_tx_payment_skey)"
default_cardano_upstream_mode() {
  if [[ "$NETWORK" == "mainnet" ]]; then
    echo "public"
  else
    echo "local"
  fi
}

CARDANO_UPSTREAM_MODE="${CARDANO_UPSTREAM_MODE:-$(default_cardano_upstream_mode)}"
PUBLIC_UPSTREAM_PEER_ADDRESS="${PUBLIC_UPSTREAM_PEER_ADDRESS:-backbone.cardano.iog.io:3001}"
PUBLIC_UPSTREAM_EXCLUDED_PROCESSES=(3-cardano-node 6-prepare-wallet)
default_tx_query_source() {
  if [[ "$CARDANO_UPSTREAM_MODE" == "public" ]]; then
    echo "koios"
  else
    echo "local"
  fi
}

TX_QUERY_SOURCE="${TX_QUERY_SOURCE:-$(default_tx_query_source)}"
START_TELEMETRY="${START_TELEMETRY:-true}"
TELEMETRY_DIR="${TELEMETRY_DIR:-$AMARU_DIR/monitoring}"
TELEMETRY_COMPOSE_OVERRIDE_FILE="${TELEMETRY_COMPOSE_OVERRIDE_FILE:-$SCRIPT_DIR/telemetry/docker-compose.yml}"
TELEMETRY_GRAFANA_URL="${TELEMETRY_GRAFANA_URL:-http://localhost}"
TELEMETRY_PROMETHEUS_URL="${TELEMETRY_PROMETHEUS_URL:-http://localhost:9090}"
OTEL_EXPORTER_OTLP_ENDPOINT="${OTEL_EXPORTER_OTLP_ENDPOINT:-http://localhost:4317}"
OTEL_EXPORTER_OTLP_METRICS_ENDPOINT="${OTEL_EXPORTER_OTLP_METRICS_ENDPOINT:-http://localhost:4318/v1/metrics}"
AMARU_MIDDLE_WITH_OPEN_TELEMETRY="${AMARU_MIDDLE_WITH_OPEN_TELEMETRY:-true}"
AMARU_DOWNSTREAM_WITH_OPEN_TELEMETRY="${AMARU_DOWNSTREAM_WITH_OPEN_TELEMETRY:-true}"
AMARU_MIDDLE_WITH_JSON_TRACES="${AMARU_MIDDLE_WITH_JSON_TRACES:-false}"
AMARU_DOWNSTREAM_WITH_JSON_TRACES="${AMARU_DOWNSTREAM_WITH_JSON_TRACES:-false}"
AMARU_TRACE_EMIT_PRIVATE="${AMARU_TRACE_EMIT_PRIVATE:-true}"
AMARU_MIDDLE_LOG="${AMARU_MIDDLE_LOG:-info,amaru::ledger::state=trace}"
AMARU_DOWNSTREAM_LOG="${AMARU_DOWNSTREAM_LOG:-info,amaru::ledger::state=trace}"
AMARU_MIDDLE_OTEL_SERVICE_NAME="${AMARU_MIDDLE_OTEL_SERVICE_NAME:-amaru-middle}"
AMARU_DOWNSTREAM_OTEL_SERVICE_NAME="${AMARU_DOWNSTREAM_OTEL_SERVICE_NAME:-amaru-downstream}"
AMARU_DEMO_TRACE="${AMARU_DEMO_TRACE:-debug,amaru::consensus=trace,amaru::stores::consensus=trace,amaru::stores::ledger=trace,amaru::stores::rocksdb=trace,amaru::mempool=trace,amaru::ledger::state=trace,amaru::ledger::context=trace,amaru::ledger::governance=trace,amaru::protocols::manager=trace,amaru::protocols::connection=trace,amaru::protocols::blockfetch::initiator=trace,amaru::protocols::mux=trace,amaru::network::connection=trace}"
AMARU_MIDDLE_TRACE="${AMARU_MIDDLE_TRACE:-$AMARU_DEMO_TRACE}"
AMARU_DOWNSTREAM_TRACE="${AMARU_DOWNSTREAM_TRACE:-$AMARU_DEMO_TRACE}"

if [[ -z "${CARDANO_NODE:-}" ]]; then
  CARDANO_NODE="$CARDANO_NODE_HOME/bin/cardano-node"
elif [[ "$CARDANO_NODE_HOME_WAS_SET" == false && ! -x "$CARDANO_NODE" ]]; then
  CARDANO_NODE="$CARDANO_NODE_HOME/bin/cardano-node"
fi
CARDANO_NODE_CONFIG_DIR="$AMARU_DIR/cardano-node-config/$NETWORK"
CARDANO_NODE_SOCKET_FILE="$RUNDIR/generated/cardano-node.socket"
CARDANO_CLI="${CARDANO_CLI:-$(command -v cardano-cli || true)}"

UPSTREAM_PORT="${UPSTREAM_PORT:-3001}"
LISTEN_PORT="${LISTEN_PORT:-4001}"
DOWNSTREAM_LISTEN_PORT="${DOWNSTREAM_LISTEN_PORT:-4002}"
DOWNSTREAM_SUBMIT_API_ADDRESS="${DOWNSTREAM_SUBMIT_API_ADDRESS:-127.0.0.1:8091}"
AMARU_MIDDLE_OTEL_SERVICE_INSTANCE_ID="${AMARU_MIDDLE_OTEL_SERVICE_INSTANCE_ID:-relay-1-middle-$LISTEN_PORT}"
AMARU_DOWNSTREAM_OTEL_SERVICE_INSTANCE_ID="${AMARU_DOWNSTREAM_OTEL_SERVICE_INSTANCE_ID:-relay-1-downstream-$DOWNSTREAM_LISTEN_PORT}"

. "$DEMO_COMMON_DIR/common.sh"
. "$DEMO_COMMON_DIR/cardano-node.sh"
. "$DEMO_COMMON_DIR/amaru.sh"
. "$DEMO_COMMON_DIR/amaru-node.sh"
. "$DEMO_COMMON_DIR/databases.sh"
. "$DEMO_COMMON_DIR/tx.sh"
. "$DEMO_COMMON_DIR/telemetry.sh"
. "$DEMO_COMMON_DIR/watch.sh"
. "$DEMO_COMMON_DIR/orchestration.sh"

validate_config() {
  [[ -n "$CARDANO_NODE_CONFIG_DIR" ]] || die "CARDANO_NODE_CONFIG_DIR must be set (directory with config.json, topology.json, etc.)"
  [[ -d "$CARDANO_NODE_CONFIG_DIR" ]] || die "CARDANO_NODE_CONFIG_DIR does not exist: $CARDANO_NODE_CONFIG_DIR"
  [[ -f "$(cardano_node_config_file)" ]] || die "cardano-node config file not found: $(cardano_node_config_file)"
  if ! public_cardano_upstream_enabled; then
    [[ -f "$(cardano_node_topology_file)" ]] || die "cardano-node topology file not found: $(cardano_node_topology_file)"
  fi
  [[ -d "$AMARU_DIR" ]] || die "AMARU_DIR does not exist: $AMARU_DIR"
  validate_network_config
  require_configured_tx
}

validate_startup_config() {
  [[ -n "$CARDANO_NODE_CONFIG_DIR" ]] || die "CARDANO_NODE_CONFIG_DIR must be set (directory with config.json, topology.json, etc.)"
  [[ -d "$AMARU_DIR" ]] || die "AMARU_DIR does not exist: $AMARU_DIR"
  require_configured_tx
}

initialize() {
  require_unscaled_process initialize
  require_runtime_processes_stopped initialize
  validate_config
  build_amaru_node_binary
  prepare_run_directories
  initialize_cardano_node_database
  echo "[initialize] initialize complete"
}

prepare_run_directories() {
  echo "[initialize] validating configuration and source databases..."
  validate_amaru_source_databases
  have rsync || die "rsync not found; cannot synchronize Amaru databases"
  echo "[initialize] ensuring log and run directories exist..."
  ensure_dirs
  echo "[initialize] clearing previous relay logs..."
  rm -f "${DEMO_LOG_FILES[@]}"

  echo "[initialize] clearing previous submit transaction artifacts..."
  rm -rf "$RUNDIR"/generated/submit-tx-* "$RUNDIR/generated/submit-tx-claims" "$RUNDIR/generated/submit-tx-active" 2>/dev/null || true
  rm -f "$RUNDIR"/generated/tx-*.body "$RUNDIR"/generated/tx-*.json "$RUNDIR"/generated/tx-*.cbor 2>/dev/null || true
  rm -f "$RUNDIR/generated/utxo.json" "$RUNDIR/generated/last-response.txt" "$RUNDIR/generated/last-response.txt.status" 2>/dev/null || true
  mkdir -p "$AMARU_MIDDLE_DATA_DIR" "$AMARU_DOWNSTREAM_DATA_DIR"
  sync_database_dir "middle chain" "$AMARU_CHAIN_SOURCE_DIR" "$AMARU_MIDDLE_DATA_DIR/chain.$NETWORK.db"
  sync_database_dir "middle ledger" "$AMARU_LEDGER_SOURCE_DIR" "$AMARU_MIDDLE_DATA_DIR/ledger.$NETWORK.db"
  sync_database_dir "downstream chain" "$AMARU_CHAIN_SOURCE_DIR" "$AMARU_DOWNSTREAM_DATA_DIR/chain.$NETWORK.db"
  sync_database_dir "downstream ledger" "$AMARU_LEDGER_SOURCE_DIR" "$AMARU_DOWNSTREAM_DATA_DIR/ledger.$NETWORK.db"
}

amaru_middle_peer_address() {
  if public_cardano_upstream_enabled; then
    echo "$PUBLIC_UPSTREAM_PEER_ADDRESS"
  else
    echo "127.0.0.1:$UPSTREAM_PORT"
  fi
}

run_amaru_middle() {
  run_amaru_node MIDDLE "$(amaru_middle_peer_address)" "0.0.0.0:$LISTEN_PORT"
}

run_amaru_downstream() {
  run_amaru_node DOWNSTREAM "127.0.0.1:$LISTEN_PORT" "0.0.0.0:$DOWNSTREAM_LISTEN_PORT" \
    --submit-api-address "$DOWNSTREAM_SUBMIT_API_ADDRESS"
}

ready_amaru_middle() {
  ready_amaru_node_listening "$AMARU_MIDDLE_LOG_FILE"
}

ready_amaru_downstream() {
  ready_amaru_submit_api "$DOWNSTREAM_SUBMIT_API_ADDRESS"
}

run_submit_tx() {
  validate_network_config
  generate_submit
}

run_submit_tx_batch() {
  validate_network_config
  submit_tx_batch "${1:-}"
}

restart_submit_tx_replicas() {
  have process-compose || die "process-compose not found"
  cd "$SCRIPT_DIR" || return
  local process
  while IFS= read -r process; do
    case "$process" in
    7-submit-tx | 7-submit-tx-[0-9]*) process-compose process restart "$process" ;;
    esac
  done < <(process-compose list)
}

run_prepare_wallet() {
  validate_network_config
  prepare_wallet
}

telemetry_urls() {
  grafana_trace_url "traces" '{ resource.service.name = "amaru-middle" && span:name = "roll_forward.process" } with (most_recent=true)'
  grafana_logs_url "logs" '{service_name=~"amaru-middle|amaru-downstream"}'
  grafana_dashboard_url "amaru-relay-mempool"
}

case "${1:-up}" in
up | start) up ;;
refresh)
  setup
  refresh_from_mithril
  ;;
down | stop) down ;;
status) status ;;
setup) setup ;;
initialize) initialize ;;
submit-tx-restart-all) restart_submit_tx_replicas ;;
prepare-wallet) run_prepare_wallet ;;
telemetry-up) telemetry_up ;;
telemetry-down) telemetry_down ;;
telemetry-reset | telemetry-clean) telemetry_reset ;;
telemetry-open | open-telemetry) open_telemetry ;;
telemetry-urls) telemetry_urls ;;
run)
  case "${2:-}" in
  setup | 0-setup) setup ;;
  telemetry-up | 8-telemetry-setup) telemetry_up ;;
  mithril-refresh | 1-mithril-refresh) run_mithril_refresh ;;
  initialize | 2-initialize) initialize ;;
  cardano-upstream | cardano-node | 3-cardano-node) run_cardano_upstream ;;
  amaru-middle | 4-amaru-middle) run_amaru_middle ;;
  amaru-downstream | 5-amaru-downstream) run_amaru_downstream ;;
  watch | 9-watch) run_watch ;;
  telemetry-open | telemetry | 8-telemetry-open) open_telemetry ;;
  prepare-wallet | 6-prepare-wallet)
    run_prepare_wallet
    exit $?
    ;;
  submit-tx | 7-submit-tx)
    run_submit_tx
    exit $?
    ;;
  submit-tx-batch | 7-submit-tx-batch)
    run_submit_tx_batch "${3:-}"
    exit $?
    ;;
  *) die "usage: $0 run {setup|telemetry-up|mithril-refresh|initialize|cardano-upstream|amaru-middle|amaru-downstream|watch|telemetry-open|submit-tx|submit-tx-batch|prepare-wallet}" ;;
  esac
  ;;
ready)
  case "${2:-}" in
  cardano-upstream | cardano-node | 3-cardano-node) ready_cardano_upstream ;;
  amaru-middle | 4-amaru-middle) ready_amaru_middle ;;
  amaru-downstream | 5-amaru-downstream) ready_amaru_downstream ;;
  *) die "usage: $0 ready {cardano-upstream|amaru-middle|amaru-downstream}" ;;
  esac
  ;;
*) die "usage: $0 {up|refresh|down|status|setup|initialize|submit-tx-restart-all|prepare-wallet|telemetry-reset|run <process>|ready <process>}" ;;
esac
