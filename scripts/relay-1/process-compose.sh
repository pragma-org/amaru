#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
AMARU_DIR="${AMARU_DIR:-$(cd "$SCRIPT_DIR/../.." && pwd)}"

LOGDIR="${LOGDIR:-/tmp/amaru-relay-1}"
RUNDIR="${RUNDIR:-$SCRIPT_DIR/run}"
NETWORK="${AMARU_NETWORK:-preprod}"
BUILD_PROFILE="${BUILD_PROFILE:-release}"
REFRESH_FROM_MITHRIL="${REFRESH_FROM_MITHRIL:-true}"
MITHRIL_REFRESH_DIR="${MITHRIL_REFRESH_DIR:-$RUNDIR/mithril-refresh}"
AMARU_MIDDLE_LOG_FILE="${AMARU_MIDDLE_LOG_FILE:-$LOGDIR/amaru-middle.log}"
AMARU_DOWNSTREAM_LOG_FILE="${AMARU_DOWNSTREAM_LOG_FILE:-$LOGDIR/amaru-downstream.log}"
DEFAULT_AMARU_CHAIN_SOURCE_DIR="$MITHRIL_REFRESH_DIR/chain.$NETWORK.db"
DEFAULT_AMARU_LEDGER_SOURCE_DIR="$MITHRIL_REFRESH_DIR/ledger.$NETWORK.db"
AMARU_CHAIN_SOURCE_DIR="${AMARU_CHAIN_SOURCE_DIR:-$DEFAULT_AMARU_CHAIN_SOURCE_DIR}"
AMARU_LEDGER_SOURCE_DIR="${AMARU_LEDGER_SOURCE_DIR:-$DEFAULT_AMARU_LEDGER_SOURCE_DIR}"
TX_PAYMENT_SKEY="${TX_PAYMENT_SKEY:-$SCRIPT_DIR/keys/payment.skey}"
TX_WAIT_FOR_SYNC="${TX_WAIT_FOR_SYNC:-true}"
CLEAR_SUBMIT_TX_CLAIMS_ON_START="${CLEAR_SUBMIT_TX_CLAIMS_ON_START:-true}"
START_TELEMETRY="${START_TELEMETRY:-true}"
TELEMETRY_COMPOSE_FILE="${TELEMETRY_COMPOSE_FILE:-$AMARU_DIR/monitoring/grafana-tempo/docker-compose.yml}"
TELEMETRY_GRAFANA_URL="${TELEMETRY_GRAFANA_URL:-http://localhost}"
TELEMETRY_PROMETHEUS_URL="${TELEMETRY_PROMETHEUS_URL:-http://localhost:9090}"
OTEL_EXPORTER_OTLP_ENDPOINT="${OTEL_EXPORTER_OTLP_ENDPOINT:-http://localhost:4317}"
OTEL_EXPORTER_OTLP_METRICS_ENDPOINT="${OTEL_EXPORTER_OTLP_METRICS_ENDPOINT:-http://localhost:4318/v1/metrics}"
AMARU_MIDDLE_WITH_OPEN_TELEMETRY="${AMARU_MIDDLE_WITH_OPEN_TELEMETRY:-true}"
AMARU_DOWNSTREAM_WITH_OPEN_TELEMETRY="${AMARU_DOWNSTREAM_WITH_OPEN_TELEMETRY:-false}"
AMARU_MIDDLE_OTEL_SERVICE_NAME="${AMARU_MIDDLE_OTEL_SERVICE_NAME:-amaru-middle}"
AMARU_MIDDLE_TRACE="${AMARU_MIDDLE_TRACE:-info,amaru::stores::consensus=trace,amaru::mempool=trace}"
AMARU_DOWNSTREAM_TRACE="${AMARU_DOWNSTREAM_TRACE:-info}"

CARDANO_NODE="${CARDANO_NODE:-}"
CARDANO_NODE_CONFIG_DIR="${CARDANO_NODE_CONFIG_DIR:-}"
CARDANO_NODE_CONFIG_FILE="${CARDANO_NODE_CONFIG_FILE:-}"
CARDANO_NODE_TOPOLOGY_FILE="${CARDANO_NODE_TOPOLOGY_FILE:-}"
CARDANO_NODE_SOCKET_FILE="${CARDANO_NODE_SOCKET_FILE:-$RUNDIR/generated/cardano-node.socket}"
CARDANO_CLI="${CARDANO_CLI:-$(command -v cardano-cli || true)}"
CARDANO_TESTNET_MAGIC="${CARDANO_TESTNET_MAGIC:-}"

UPSTREAM_PORT="${UPSTREAM_PORT:-3001}"
LISTEN_PORT="${LISTEN_PORT:-4001}"
DOWNSTREAM_LISTEN_PORT="${DOWNSTREAM_LISTEN_PORT:-4002}"
DOWNSTREAM_SUBMIT_API_ADDRESS="${DOWNSTREAM_SUBMIT_API_ADDRESS:-127.0.0.1:8091}"
AMARU_MIDDLE_OTEL_SERVICE_INSTANCE_ID="${AMARU_MIDDLE_OTEL_SERVICE_INSTANCE_ID:-relay-1-middle-$LISTEN_PORT}"

. "$AMARU_DIR/scripts/demo/common.sh"
. "$AMARU_DIR/scripts/demo/cardano-node.sh"
. "$AMARU_DIR/scripts/demo/amaru.sh"
. "$AMARU_DIR/scripts/demo/tx.sh"

validate_config() {
  require_cardano_node
  [[ -n "$CARDANO_NODE_CONFIG_DIR" ]] || die "CARDANO_NODE_CONFIG_DIR must be set (directory with config.json, topology.json, etc.)"
  [[ -d "$CARDANO_NODE_CONFIG_DIR" ]] || die "CARDANO_NODE_CONFIG_DIR does not exist: $CARDANO_NODE_CONFIG_DIR"
  [[ -f "$(cardano_node_config_file)" ]] || die "cardano-node config file not found: $(cardano_node_config_file)"
  [[ -f "$(cardano_node_topology_file)" ]] || die "cardano-node topology file not found: $(cardano_node_topology_file)"
  [[ -d "$AMARU_DIR" ]] || die "AMARU_DIR does not exist: $AMARU_DIR"
  require_configured_tx
}

validate_amaru_source_databases() {
  [[ -d "$AMARU_CHAIN_SOURCE_DIR" ]] ||
    die "AMARU_CHAIN_SOURCE_DIR does not exist: $AMARU_CHAIN_SOURCE_DIR; run ./process-compose.sh refresh first"
  [[ -d "$AMARU_LEDGER_SOURCE_DIR" ]] ||
    die "AMARU_LEDGER_SOURCE_DIR does not exist: $AMARU_LEDGER_SOURCE_DIR; run ./process-compose.sh refresh first"
  if [[ "$AMARU_CHAIN_SOURCE_DIR" == "$DEFAULT_AMARU_CHAIN_SOURCE_DIR" &&
    "$AMARU_LEDGER_SOURCE_DIR" == "$DEFAULT_AMARU_LEDGER_SOURCE_DIR" &&
    ! -f "$MITHRIL_REFRESH_DIR/.mithril-refresh.json" ]]; then
    die "default refreshed databases are incomplete: $MITHRIL_REFRESH_DIR/.mithril-refresh.json is missing; wait for ./process-compose.sh refresh to finish"
  fi
}

setup() {
  require_unscaled_process setup
  require_runtime_processes_stopped setup
  validate_config
  if truthy "$REFRESH_FROM_MITHRIL"; then
    refresh_from_mithril
  fi
  build_amaru_node_binary
  prepare_run_directories
}

require_unscaled_process() {
  local process_name="$1" replica_num="${PC_REPLICA_NUM:-0}"
  if [[ ! "$replica_num" =~ ^0+$ ]]; then
    die "$process_name cannot be scaled: replica $replica_num would mutate shared demo directories"
  fi
}

require_runtime_processes_stopped() {
  local process_name="$1"
  if pgrep -f "cardano-node.*--socket-path $(cardano_node_socket_file)" >/dev/null 2>&1 ||
    pgrep -f "target/$(target_profile_dir)/amaru.*$RUNDIR/amaru" >/dev/null 2>&1; then
    die "$process_name cannot run while demo runtime processes are active; run ./process-compose.sh down first"
  fi
}

prepare_run_directories() {
  echo "[setup] validating configuration and source databases..."
  validate_config
  validate_amaru_source_databases
  echo "[setup] ensuring log and run directories exist..."
  ensure_dirs
  echo "[setup] clearing previous relay logs from $LOGDIR..."
  rm -f "$LOGDIR"/*.log 2>/dev/null || true
  echo "[setup] clearing previous submit transaction artifacts..."
  rm -rf "$RUNDIR"/generated/submit-tx-* "$RUNDIR/generated/submit-tx-claims" 2>/dev/null || true
  rm -f "$RUNDIR"/generated/tx-*.body "$RUNDIR"/generated/tx-*.json "$RUNDIR"/generated/tx-*.cbor 2>/dev/null || true
  rm -f "$RUNDIR/generated/utxo.json" "$RUNDIR/generated/last-response.txt" "$RUNDIR/generated/last-response.txt.status" 2>/dev/null || true
  echo "[setup] recreating isolated Amaru run directories under $RUNDIR..."
  rm -rf "$RUNDIR/amaru" "$RUNDIR/amaru-downstream"
  mkdir -p "$RUNDIR/amaru" "$RUNDIR/amaru-downstream"
  echo "[setup] copying middle chain database: $AMARU_CHAIN_SOURCE_DIR -> $RUNDIR/amaru/chain.$NETWORK.db"
  cp -r "$AMARU_CHAIN_SOURCE_DIR" "$RUNDIR/amaru/chain.$NETWORK.db"
  echo "[setup] copying middle ledger database: $AMARU_LEDGER_SOURCE_DIR -> $RUNDIR/amaru/ledger.$NETWORK.db"
  cp -r "$AMARU_LEDGER_SOURCE_DIR" "$RUNDIR/amaru/ledger.$NETWORK.db"
  echo "[setup] copying downstream chain database: $AMARU_CHAIN_SOURCE_DIR -> $RUNDIR/amaru-downstream/chain.$NETWORK.db"
  cp -r "$AMARU_CHAIN_SOURCE_DIR" "$RUNDIR/amaru-downstream/chain.$NETWORK.db"
  echo "[setup] copying downstream ledger database: $AMARU_LEDGER_SOURCE_DIR -> $RUNDIR/amaru-downstream/ledger.$NETWORK.db"
  cp -r "$AMARU_LEDGER_SOURCE_DIR" "$RUNDIR/amaru-downstream/ledger.$NETWORK.db"
  echo "[setup] setup complete"
}

refresh_from_mithril() {
  cd "$AMARU_DIR"
  AMARU_NETWORK="$NETWORK" BUILD_PROFILE="$BUILD_PROFILE" STAGING_DIR="$MITHRIL_REFRESH_DIR" INSTALL=false ./scripts/refresh-from-mithril
}

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
  cd "$AMARU_DIR"
  echo "[setup] building Amaru node binary with BUILD_PROFILE=$BUILD_PROFILE..."
  cargo build --profile "$BUILD_PROFILE" --bin amaru
}

ensure_amaru_node_binary() {
  [[ -x "$(amaru_node_binary)" ]] || build_amaru_node_binary
}

run_mithril_refresh() {
  validate_config
  if truthy "$REFRESH_FROM_MITHRIL"; then
    refresh_from_mithril
  else
    echo "[mithril-refresh] skipped because REFRESH_FROM_MITHRIL=false"
  fi
}

run_cardano_upstream() {
  require_cardano_node
  prepare_cardano_node_topology_file
  mkdir -p "$(dirname "$(cardano_node_socket_file)")"
  rm -f "$(cardano_node_socket_file)"
  "$CARDANO_NODE" run \
    --config "$(cardano_node_config_file)" \
    --topology "$(cardano_node_effective_topology_file)" \
    --database-path "$CARDANO_NODE_CONFIG_DIR/db" \
    --socket-path "$(cardano_node_socket_file)" \
    --port "$UPSTREAM_PORT" \
    2>&1 | tee "$LOGDIR/cardano-upstream.log"
}

run_amaru_middle() {
  cd "$AMARU_DIR"
  export AMARU_WITH_OPEN_TELEMETRY="$AMARU_MIDDLE_WITH_OPEN_TELEMETRY"
  export AMARU_TRACE="$AMARU_MIDDLE_TRACE"
  export OTEL_SERVICE_NAME="$AMARU_MIDDLE_OTEL_SERVICE_NAME"
  export OTEL_SERVICE_INSTANCE_ID="$AMARU_MIDDLE_OTEL_SERVICE_INSTANCE_ID"
  export OTEL_EXPORTER_OTLP_ENDPOINT
  export OTEL_EXPORTER_OTLP_METRICS_ENDPOINT
  ulimit -n 65536
  ensure_amaru_node_binary
  "$(amaru_node_binary)" --with-json-traces run \
    --peer-address "127.0.0.1:$UPSTREAM_PORT" \
    --listen-address "0.0.0.0:$LISTEN_PORT" \
    --chain-dir "$RUNDIR/amaru/chain.$NETWORK.db" \
    --ledger-dir "$RUNDIR/amaru/ledger.$NETWORK.db" \
    2>&1 | tee "$AMARU_MIDDLE_LOG_FILE"
}

run_amaru_downstream() {
  cd "$AMARU_DIR"
  export AMARU_WITH_OPEN_TELEMETRY="$AMARU_DOWNSTREAM_WITH_OPEN_TELEMETRY"
  export AMARU_TRACE="$AMARU_DOWNSTREAM_TRACE"
  ulimit -n 65536
  ensure_amaru_node_binary
  "$(amaru_node_binary)" --with-json-traces run \
    --peer-address "127.0.0.1:$LISTEN_PORT" \
    --listen-address "0.0.0.0:$DOWNSTREAM_LISTEN_PORT" \
    --submit-api-address "$DOWNSTREAM_SUBMIT_API_ADDRESS" \
    --chain-dir "$RUNDIR/amaru-downstream/chain.$NETWORK.db" \
    --ledger-dir "$RUNDIR/amaru-downstream/ledger.$NETWORK.db" \
    2>&1 | tee "$AMARU_DOWNSTREAM_LOG_FILE"
}

ready_cardano_upstream() {
  require_cardano_cli
  have jq || exit 1
  [[ -S "$(cardano_node_socket_file)" ]] || exit 1
  cardano_node_tip_slot "$(network_magic)" >/dev/null 2>&1
}

ready_amaru_middle() {
  true
}

ready_amaru_downstream() {
  curl -sS -o /dev/null --max-time "${SUBMIT_API_READY_TIMEOUT_SECONDS:-2}" "http://$DOWNSTREAM_SUBMIT_API_ADDRESS/" >/dev/null 2>&1
}

colorize_watch_logs() {
  local color="${WATCH_COLOR:-always}"
  if [[ "$color" == "never" || "$color" == "false" ]]; then
    color=false
  else
    color=true
  fi

  awk \
    -v color="$color" \
    -v cardano_log="$LOGDIR/cardano-upstream.log" \
    -v middle_log="$AMARU_MIDDLE_LOG_FILE" \
    -v downstream_log="$AMARU_DOWNSTREAM_LOG_FILE" \
    -v submit_log="$LOGDIR/submit-tx.log" '
      function paint(code, text) {
        return color == "true" ? sprintf("%c[%sm%s%c[0m", 27, code, text, 27) : text
      }

      function source_for(path) {
        if (path == cardano_log) return "cardano-upstream"
        if (path == middle_log) return "amaru-middle"
        if (path == downstream_log) return "amaru-downstream"
        if (path == submit_log) return "submit-tx"
        return "log"
      }

      BEGIN {
        source = "log"
      }

      /^==> .* <==$/ {
        current = $0
        sub(/^==> /, "", current)
        sub(/ <==$/, "", current)
        source = source_for(current)
        next
      }

      {
        line = $0
        lower = tolower(line)
        style = ""
        marker = ""

        if (lower ~ /error|rejected|giving up|failed|non-retryable/) {
          style = "1;31"
        } else if (lower ~ /warn/) {
          style = "1;33"
        } else if (source == "submit-tx" && lower ~ /submitting|building transaction|response: http 202|selected utxo/) {
          style = "1;36"
          marker = ">>> TX >>> "
        } else if (source == "amaru-downstream" && lower ~ /transaction accepted into mempool/) {
          style = "1;36"
          marker = ">>> TX >>> "
        } else if (source == "amaru-middle" && lower ~ /transaction accepted into mempool/) {
          style = "1;36"
          marker = ">>> TX >>> "
        } else if (source == "cardano-upstream" && line ~ /TraceMempoolAddedTx/) {
          style = "1;36"
          marker = ">>> TX >>> "
        } else if (line ~ /TxSubmissionOutbound(RecvMsgRequestTxs|SendMsgReplyTxs)/) {
          style = "1;36"
          marker = ">>> TX >>> "
        }

        label_style = source == "cardano-upstream" ? "33" : source == "amaru-middle" ? "35" : source == "amaru-downstream" ? "32" : source == "submit-tx" ? "36" : "37"
        label = sprintf("[%-17s]", source)
        label = paint(label_style, label)
        message = marker line
        print label " " (style == "" ? message : paint(style, message))
        fflush()
      }'
}

run_watch() {
  tail -n +1 -F \
    "$LOGDIR/cardano-upstream.log" \
    "$AMARU_MIDDLE_LOG_FILE" \
    "$AMARU_DOWNSTREAM_LOG_FILE" \
    "$LOGDIR/submit-tx.log" \
    2>/dev/null \
    | colorize_watch_logs || true
}

run_submit_tx() {
  local replica_num="${PC_REPLICA_NUM:-0}"
  if truthy "$CLEAR_SUBMIT_TX_CLAIMS_ON_START" && [[ "$replica_num" =~ ^0+$ ]]; then
    echo "[submit-tx] clearing stale submit-tx claims from $RUNDIR/generated/submit-tx-claims"
    rm -rf "$RUNDIR/generated/submit-tx-claims"
  fi
  generate_submit
}

telemetry_compose() {
  have docker || die "docker not found; install Docker or set START_TELEMETRY=false"
  [[ -f "$TELEMETRY_COMPOSE_FILE" ]] || die "telemetry compose file not found: $TELEMETRY_COMPOSE_FILE"
  docker compose -f "$TELEMETRY_COMPOSE_FILE" "$@"
}

telemetry_up() {
  if ! truthy "$START_TELEMETRY"; then
    echo "[telemetry] skipped because START_TELEMETRY=false"
    return 0
  fi

  echo "[telemetry] starting Grafana, Tempo, Prometheus, and the OTLP collector..."
  telemetry_compose up -d
  echo "[telemetry] Grafana: $TELEMETRY_GRAFANA_URL"
  echo "[telemetry] Prometheus: $TELEMETRY_PROMETHEUS_URL"
}

telemetry_down() {
  telemetry_compose down
}

urlencode() {
  if have jq; then
    jq -nr --arg value "$1" '$value|@uri'
  else
    die "jq not found; cannot encode telemetry URLs"
  fi
}

grafana_trace_url() {
  local pane_id="$1" query="$2" panes
  panes="$(
    jq -cn \
      --arg pane_id "$pane_id" \
      --arg query "$query" \
      '{
        ($pane_id): {
          datasource: "tempo",
          queries: [
            {
              refId: "A",
              datasource: {type: "tempo", uid: "tempo"},
              queryType: "traceql",
              query: $query
            }
          ],
          range: {from: "now-15m", to: "now"}
        }
      }'
  )"
  printf '%s/explore?orgId=1&schemaVersion=1&refresh=5s&panes=%s\n' "$TELEMETRY_GRAFANA_URL" "$(urlencode "$panes")"
}

grafana_metric_url() {
  local query="$1"
  local panes
  panes="$(
    jq -cn \
      --arg query "$query" \
      '{
        metrics: {
          datasource: "prometheus",
          queries: [
            {
              refId: "A",
              datasource: {type: "prometheus", uid: "prometheus"},
              expr: $query,
              range: true,
              instant: false
            }
          ],
          range: {from: "now-15m", to: "now"}
        }
      }'
  )"
  printf '%s/explore?orgId=1&schemaVersion=1&refresh=5s&panes=%s\n' "$TELEMETRY_GRAFANA_URL" "$(urlencode "$panes")"
}

telemetry_urls() {
  grafana_trace_url "headers" '{ resource.service.name = "amaru-middle" && name = "store_header" }'
  grafana_trace_url "transactions" '{ resource.service.name = "amaru-middle" && (name = "tx_received" || name = "tx_accepted") }'
  grafana_metric_url 'sum by (origin, result) (cardano_node_metrics_mempoolTxInsertionsNum_int_total{exported_job="amaru-middle"}) or vector(0)'
}

open_telemetry() {
  local url
  if have open; then
    telemetry_urls | while IFS= read -r url; do
      open "$url"
    done
  elif have xdg-open; then
    telemetry_urls | while IFS= read -r url; do
      xdg-open "$url"
    done
  else
    telemetry_urls
  fi
}

validate_up() {
  validate_config
  if ! truthy "$REFRESH_FROM_MITHRIL"; then
    validate_amaru_source_databases
  fi
}

up() {
  have process-compose || die "process-compose not found"
  validate_up
  telemetry_up
  cd "$SCRIPT_DIR"
  exec process-compose -f process-compose.yaml up
}

down() {
  have process-compose || die "process-compose not found"
  cd "$SCRIPT_DIR"
  process-compose down
  if truthy "$START_TELEMETRY"; then
    telemetry_down
  fi
}

status() {
  have process-compose || die "process-compose not found"
  cd "$SCRIPT_DIR"
  process-compose list
}

case "${1:-up}" in
  up | start) up ;;
  refresh) refresh_from_mithril ;;
  down | stop) down ;;
  status) status ;;
  setup) setup ;;
  telemetry-up) telemetry_up ;;
  telemetry-down) telemetry_down ;;
  telemetry-open | open-telemetry) open_telemetry ;;
  telemetry-urls) telemetry_urls ;;
  run)
    case "${2:-}" in
      mithril-refresh) run_mithril_refresh ;;
      setup) setup ;;
      cardano-upstream) run_cardano_upstream ;;
      amaru-middle) run_amaru_middle ;;
      amaru-downstream) run_amaru_downstream ;;
      watch) run_watch ;;
      telemetry-open) open_telemetry ;;
      submit-tx) run_submit_tx; exit $? ;;
      *) die "usage: $0 run {mithril-refresh|setup|cardano-upstream|amaru-middle|amaru-downstream|watch|telemetry-open|submit-tx}" ;;
    esac
    ;;
  ready)
    case "${2:-}" in
      cardano-upstream) ready_cardano_upstream ;;
      amaru-middle) ready_amaru_middle ;;
      amaru-downstream) ready_amaru_downstream ;;
      *) die "usage: $0 ready {cardano-upstream|amaru-middle|amaru-downstream}" ;;
    esac
    ;;
  *) die "usage: $0 {up|refresh|down|status|setup|run <process>|ready <process>}" ;;
esac
