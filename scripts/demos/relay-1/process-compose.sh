#!/bin/sh
if [ -z "${BASH_VERSION:-}" ] || shopt -qo posix 2>/dev/null; then
  exec bash "$0" "$@"
fi

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
BOOTSTRAP_AMARU_DATABASES="${BOOTSTRAP_AMARU_DATABASES:-auto}"
# Matches the node's own default. Retaining every historical snapshot costs about 2 GB per epoch on
# mainnet, twice over, because each node gets its own copy of the databases.
AMARU_MAX_EXTRA_LEDGER_SNAPSHOTS="${AMARU_MAX_EXTRA_LEDGER_SNAPSHOTS:-0}"
CARDANO_NODE_RELEASE_VERSION="${CARDANO_NODE_RELEASE_VERSION:-11.0.1}"
DEFAULT_CARDANO_NODE_HOME="$LOGDIR/cardano-node-$CARDANO_NODE_RELEASE_VERSION"
CARDANO_NODE_HOME_WAS_SET=false
if [[ -n "${CARDANO_NODE_HOME:-}" ]]; then
  CARDANO_NODE_HOME_WAS_SET=true
else
  CARDANO_NODE_HOME="$DEFAULT_CARDANO_NODE_HOME"
fi
AMARU_BOOTSTRAP_DIR="${AMARU_BOOTSTRAP_DIR:-$RUNDIR/bootstrap}"
AMARU_BOOTSTRAP_LOG_FILE="${AMARU_BOOTSTRAP_LOG_FILE:-$LOGDIR/bootstrap.log}"
AMARU_BOOTSTRAP_EPOCH="${AMARU_BOOTSTRAP_EPOCH:-}"
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
AMARU_CHAIN_SOURCE_DIR="$AMARU_BOOTSTRAP_DIR/chain.$NETWORK.db"
AMARU_LEDGER_SOURCE_DIR="$AMARU_BOOTSTRAP_DIR/ledger.$NETWORK.db"

# The wallet lives under RUNDIR so it follows the run directory wherever that points: on the host
# that is this directory's run/, and in the container it is the data volume, where a key put there
# survives the image.
default_tx_payment_skey() {
  if [[ -f "$RUNDIR/$NETWORK-wallet/payment.skey" ]]; then
    echo "$RUNDIR/$NETWORK-wallet/payment.skey"
  elif [[ -f "$SCRIPT_DIR/keys/$NETWORK/payment.skey" ]]; then
    echo "$SCRIPT_DIR/keys/$NETWORK/payment.skey"
  else
    echo "$SCRIPT_DIR/keys/payment.skey"
  fi
}

TX_PAYMENT_SKEY="${TX_PAYMENT_SKEY:-$(default_tx_payment_skey)}"

# A relative path is made absolute here, while the working directory is still the one the wrapper was
# invoked from: the commands below change into the demo directory before starting anything, so a
# relative path would otherwise resolve against that instead of against what the caller typed.
# Only an existing file is rewritten, which leaves inline key material and absolute paths untouched.
if [[ "$TX_PAYMENT_SKEY" != /* && -f "$TX_PAYMENT_SKEY" ]]; then
  TX_PAYMENT_SKEY="$(cd "$(dirname "$TX_PAYMENT_SKEY")" && pwd)/$(basename "$TX_PAYMENT_SKEY")"
fi

# Public well-known relays per network; custom testnets have no public relay, so they
# fall back to a local cardano-node address and require CARDANO_UPSTREAM_MODE=local.
default_public_upstream_peer_address() {
  case "$NETWORK" in
    mainnet) echo "backbone.cardano.iog.io:3001" ;;
    preprod) echo "preprod-node.play.dev.cardano.org:3001" ;;
    preview) echo "preview-node.play.dev.cardano.org:3001" ;;
    *) echo "127.0.0.1:3001" ;;
  esac
}

CARDANO_UPSTREAM_MODE="${CARDANO_UPSTREAM_MODE:-public}"
PUBLIC_UPSTREAM_PEER_ADDRESS="${PUBLIC_UPSTREAM_PEER_ADDRESS:-$(default_public_upstream_peer_address)}"
PUBLIC_UPSTREAM_EXCLUDED_PROCESSES=(3-cardano-node)
default_tx_query_source() {
  if [[ "$CARDANO_UPSTREAM_MODE" == "public" ]]; then
    echo "koios"
  else
    echo "local"
  fi
}

TX_QUERY_SOURCE="${TX_QUERY_SOURCE:-$(default_tx_query_source)}"
TELEMETRY_GRAFANA_URL="${TELEMETRY_GRAFANA_URL:-http://localhost}"
export AMARU_RELAY_LOGDIR="$LOGDIR"
export AMARU_MAX_EXTRA_LEDGER_SNAPSHOTS
# The shared monitoring stack (monitoring/docker-compose.yml) is started and stopped
# independently of the demo; the nodes only export to it. All three exporters speak OTLP over
# gRPC, so both endpoints point at the collector's 4317 receiver.
OTEL_EXPORTER_OTLP_ENDPOINT="${OTEL_EXPORTER_OTLP_ENDPOINT:-http://localhost:4317}"
OTEL_EXPORTER_OTLP_METRICS_ENDPOINT="${OTEL_EXPORTER_OTLP_METRICS_ENDPOINT:-http://localhost:4317}"
# auto exports only when the collector answers, so running without the monitoring stack does
# not turn into an export error per second. Resolved once the helpers are sourced, below.
AMARU_DEMO_WITH_OPEN_TELEMETRY="${AMARU_DEMO_WITH_OPEN_TELEMETRY:-auto}"
AMARU_MIDDLE_WITH_JSON_TRACES="${AMARU_MIDDLE_WITH_JSON_TRACES:-false}"
AMARU_DOWNSTREAM_WITH_JSON_TRACES="${AMARU_DOWNSTREAM_WITH_JSON_TRACES:-false}"
AMARU_TRACE_EMIT_PRIVATE="${AMARU_TRACE_EMIT_PRIVATE:-true}"
AMARU_MIDDLE_LOG="${AMARU_MIDDLE_LOG:-info}"
AMARU_DOWNSTREAM_LOG="${AMARU_DOWNSTREAM_LOG:-info}"
AMARU_MIDDLE_OTEL_SERVICE_NAME="${AMARU_MIDDLE_OTEL_SERVICE_NAME:-amaru-middle}"
AMARU_DOWNSTREAM_OTEL_SERVICE_NAME="${AMARU_DOWNSTREAM_OTEL_SERVICE_NAME:-amaru-downstream}"
# Trace filter for what the nodes export, not for what they log. The spans the Grafana
# dashboards query (roll_forward.process and the rest of the consensus pipeline) are emitted at
# debug level, so an `info` filter would fill Prometheus and Loki but leave Tempo empty.
# `amaru=trace` rather than a list of targets: naming them individually reads as precise but silently
# drops whole subsystems, because the spans of one subsystem are spread over several targets. Ledger
# work, for instance, spans amaru::ledger::state, ::block and the validation-context targets, so a
# filter naming only ::state hides most of a block's validation while looking correct.
AMARU_DEMO_TRACE="${AMARU_DEMO_TRACE:-info,amaru=trace}"
AMARU_MIDDLE_TRACE="${AMARU_MIDDLE_TRACE:-$AMARU_DEMO_TRACE}"
AMARU_DOWNSTREAM_TRACE="${AMARU_DOWNSTREAM_TRACE:-$AMARU_DEMO_TRACE}"

if [[ -z "${CARDANO_NODE:-}" ]]; then
  CARDANO_NODE="$CARDANO_NODE_HOME/bin/cardano-node"
elif [[ "$CARDANO_NODE_HOME_WAS_SET" == false && ! -x "$CARDANO_NODE" ]]; then
  CARDANO_NODE="$CARDANO_NODE_HOME/bin/cardano-node"
fi
CARDANO_NODE_CONFIG_DIR="$AMARU_DIR/cardano-node-config/$NETWORK"
CARDANO_NODE_SOCKET_FILE="$RUNDIR/generated/cardano-node.socket"
# cardano-cli comes from its own pinned release, downloaded by setup; local upstream mode also
# gets one inside the cardano-node release, and whatever is on PATH wins over both.
CARDANO_CLI_RELEASE_VERSION="${CARDANO_CLI_RELEASE_VERSION:-11.2.1.0}"
CARDANO_CLI_HOME="${CARDANO_CLI_HOME:-$LOGDIR/cardano-cli-$CARDANO_CLI_RELEASE_VERSION}"
# A bare command name is resolved through PATH here, because everything downstream tests
# CARDANO_CLI with `-x`, which only ever succeeds for a path: an explicitly named `cardano-cli`
# would otherwise download the pinned release and then be rejected as not executable. A name that
# PATH does not resolve falls back to the pinned release, exactly as an unset value does.
CARDANO_CLI="${CARDANO_CLI:-cardano-cli}"
if [[ "$CARDANO_CLI" != */* ]]; then
  CARDANO_CLI="$(command -v "$CARDANO_CLI" || echo "$CARDANO_CLI_HOME/bin/cardano-cli")"
fi

UPSTREAM_PORT="${UPSTREAM_PORT:-3001}"
LISTEN_PORT="${LISTEN_PORT:-4001}"
DOWNSTREAM_LISTEN_PORT="${DOWNSTREAM_LISTEN_PORT:-4002}"
DOWNSTREAM_SUBMIT_API_ADDRESS="${DOWNSTREAM_SUBMIT_API_ADDRESS:-127.0.0.1:8091}"
MIDDLE_SUBMIT_API_ADDRESS="${MIDDLE_SUBMIT_API_ADDRESS:-127.0.0.1:8090}"
# Where the submit-tx processes post transactions; point it at MIDDLE_SUBMIT_API_ADDRESS to
# exercise submission through the middle relay instead.
TX_SUBMIT_API_ADDRESS="${TX_SUBMIT_API_ADDRESS:-$DOWNSTREAM_SUBMIT_API_ADDRESS}"
AMARU_MIDDLE_OTEL_SERVICE_INSTANCE_ID="${AMARU_MIDDLE_OTEL_SERVICE_INSTANCE_ID:-relay-1-middle-$LISTEN_PORT}"
AMARU_DOWNSTREAM_OTEL_SERVICE_INSTANCE_ID="${AMARU_DOWNSTREAM_OTEL_SERVICE_INSTANCE_ID:-relay-1-downstream-$DOWNSTREAM_LISTEN_PORT}"

. "$DEMO_COMMON_DIR/common.sh"
. "$DEMO_COMMON_DIR/cardano-cli.sh"
. "$DEMO_COMMON_DIR/cardano-node.sh"
. "$DEMO_COMMON_DIR/amaru.sh"
. "$DEMO_COMMON_DIR/amaru-node.sh"
. "$DEMO_COMMON_DIR/databases.sh"
. "$DEMO_COMMON_DIR/tx.sh"
. "$DEMO_COMMON_DIR/telemetry.sh"
. "$DEMO_COMMON_DIR/watch.sh"
. "$DEMO_COMMON_DIR/orchestration.sh"

AMARU_MIDDLE_WITH_OPEN_TELEMETRY="${AMARU_MIDDLE_WITH_OPEN_TELEMETRY:-$(resolve_open_telemetry "$AMARU_DEMO_WITH_OPEN_TELEMETRY")}"
AMARU_DOWNSTREAM_WITH_OPEN_TELEMETRY="${AMARU_DOWNSTREAM_WITH_OPEN_TELEMETRY:-$AMARU_MIDDLE_WITH_OPEN_TELEMETRY}"

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
  validate_configured_tx_inputs
}

initialize() {
  require_unscaled_process initialize
  require_runtime_processes_stopped initialize
  validate_config
  ensure_amaru_node_binary
  prepare_run_directories
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
  run_amaru_node MIDDLE "$(amaru_middle_peer_address)" "0.0.0.0:$LISTEN_PORT" \
    --submit-api-address "$MIDDLE_SUBMIT_API_ADDRESS"
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
  grafana_dashboard_panel_url "amaru-relay-consensus-perf" 7
  grafana_dashboard_url "amaru-relay-mempool"
  grafana_dashboard_url "amaru-relay-consensus-perf"
}

case "${1:-up}" in
up | start) up ;;
refresh)
  BOOTSTRAP_AMARU_DATABASES=true FORCE_REFRESH=true run_bootstrap
  ;;
down | stop) down ;;
status) status ;;
setup) setup ;;
initialize) initialize ;;
submit-tx-restart-all) restart_submit_tx_replicas ;;
prepare-wallet) run_prepare_wallet ;;
telemetry-open | open-telemetry) open_telemetry ;;
telemetry-urls) telemetry_urls ;;
run)
  case "${2:-}" in
  setup | 0-setup) setup ;;
  bootstrap | 1-bootstrap) run_bootstrap ;;
  initialize | 2-initialize) initialize ;;
  cardano-upstream | cardano-node | 3-cardano-node) run_cardano_upstream ;;
  amaru-middle | 4-amaru-middle) run_amaru_middle ;;
  amaru-downstream | 5-amaru-downstream) run_amaru_downstream ;;
  watch | 9-watch) run_watch ;;
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
  *) die "usage: $0 run {setup|bootstrap|initialize|cardano-upstream|amaru-middle|amaru-downstream|watch|submit-tx|submit-tx-batch|prepare-wallet}" ;;
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
*) die "usage: $0 {up|refresh|down|status|setup|initialize|submit-tx-restart-all|prepare-wallet|telemetry-open|telemetry-urls|run <process>|ready <process>}" ;;
esac
