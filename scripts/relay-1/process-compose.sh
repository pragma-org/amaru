#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
AMARU_DIR="${AMARU_DIR:-$(cd "$SCRIPT_DIR/../.." && pwd)}"

LOGDIR="${LOGDIR:-/tmp/amaru-relay-1}"
RUNDIR="${RUNDIR:-$SCRIPT_DIR/run}"
NETWORK="${AMARU_NETWORK:-preprod}"
BUILD_PROFILE="${BUILD_PROFILE:-dev}"
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
OTEL_METRIC_EXPORT_INTERVAL_MS="${OTEL_METRIC_EXPORT_INTERVAL_MS:-1000}"
AMARU_MIDDLE_WITH_OPEN_TELEMETRY="${AMARU_MIDDLE_WITH_OPEN_TELEMETRY:-true}"
AMARU_DOWNSTREAM_WITH_OPEN_TELEMETRY="${AMARU_DOWNSTREAM_WITH_OPEN_TELEMETRY:-true}"
AMARU_MIDDLE_WITH_JSON_TRACES="${AMARU_MIDDLE_WITH_JSON_TRACES:-false}"
AMARU_DOWNSTREAM_WITH_JSON_TRACES="${AMARU_DOWNSTREAM_WITH_JSON_TRACES:-false}"
AMARU_MIDDLE_LOG="${AMARU_MIDDLE_LOG:-info}"
AMARU_DOWNSTREAM_LOG="${AMARU_DOWNSTREAM_LOG:-info}"
AMARU_MIDDLE_OTEL_SERVICE_NAME="${AMARU_MIDDLE_OTEL_SERVICE_NAME:-amaru-middle}"
AMARU_DOWNSTREAM_OTEL_SERVICE_NAME="${AMARU_DOWNSTREAM_OTEL_SERVICE_NAME:-amaru-downstream}"
AMARU_MIDDLE_TRACE="${AMARU_MIDDLE_TRACE:-info,amaru::consensus=trace,amaru::stores::consensus=trace,amaru::stores::ledger=trace,amaru::stores::rocksdb=trace,amaru::mempool=trace,amaru::ledger::state=trace,amaru::ledger::context=trace,amaru::ledger::governance=trace,amaru::protocols::manager=trace}"
AMARU_DOWNSTREAM_TRACE="${AMARU_DOWNSTREAM_TRACE:-info,amaru::consensus=trace,amaru::stores::consensus=trace,amaru::stores::ledger=trace,amaru::stores::rocksdb=trace,amaru::mempool=trace,amaru::ledger::state=trace,amaru::ledger::context=trace,amaru::ledger::governance=trace,amaru::protocols::manager=trace}"

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
AMARU_DOWNSTREAM_OTEL_SERVICE_INSTANCE_ID="${AMARU_DOWNSTREAM_OTEL_SERVICE_INSTANCE_ID:-relay-1-downstream-$DOWNSTREAM_LISTEN_PORT}"

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
  validate_amaru_source_databases
  have rsync || die "rsync not found; cannot synchronize Amaru databases"
  echo "[setup] ensuring log and run directories exist..."
  ensure_dirs
  echo "[setup] clearing previous relay logs from $LOGDIR..."
  rm -f "$LOGDIR"/*.log 2>/dev/null || true
  echo "[setup] clearing previous submit transaction artifacts..."
  rm -rf "$RUNDIR"/generated/submit-tx-* "$RUNDIR/generated/submit-tx-claims" "$RUNDIR/generated/submit-tx-active" 2>/dev/null || true
  rm -f "$RUNDIR"/generated/tx-*.body "$RUNDIR"/generated/tx-*.json "$RUNDIR"/generated/tx-*.cbor 2>/dev/null || true
  rm -f "$RUNDIR/generated/utxo.json" "$RUNDIR/generated/last-response.txt" "$RUNDIR/generated/last-response.txt.status" 2>/dev/null || true
  mkdir -p "$RUNDIR/amaru" "$RUNDIR/amaru-downstream"
  sync_database_dir "middle chain" "$AMARU_CHAIN_SOURCE_DIR" "$RUNDIR/amaru/chain.$NETWORK.db"
  sync_database_dir "middle ledger" "$AMARU_LEDGER_SOURCE_DIR" "$RUNDIR/amaru/ledger.$NETWORK.db"
  sync_database_dir "downstream chain" "$AMARU_CHAIN_SOURCE_DIR" "$RUNDIR/amaru-downstream/chain.$NETWORK.db"
  sync_database_dir "downstream ledger" "$AMARU_LEDGER_SOURCE_DIR" "$RUNDIR/amaru-downstream/ledger.$NETWORK.db"
  echo "[setup] setup complete"
}

sync_database_dir() {
  local label="$1" source="$2" destination="$3" source_marker destination_marker
  source_marker="$(database_source_marker_file "$source")"
  destination_marker="$destination/.relay-1-source.json"
  if [[ -f "$source_marker" && -f "$destination_marker" ]] && cmp -s "$source_marker" "$destination_marker"; then
    echo "[setup] $label database unchanged; skipping sync"
    return 0
  fi

  echo "[setup] synchronizing $label database: $source -> $destination"
  mkdir -p "$destination"
  rsync -a --delete "$source"/ "$destination"/
  if [[ -f "$source_marker" ]]; then
    cp "$source_marker" "$destination_marker"
  else
    rm -f "$destination_marker"
  fi
}

mark_database_dir_dirty() {
  local directory="$1"
  rm -f "$directory/.relay-1-source.json"
}

database_source_marker_file() {
  local source="$1"
  if [[ "$source" == "$AMARU_CHAIN_SOURCE_DIR" || "$source" == "$AMARU_LEDGER_SOURCE_DIR" ]] &&
    [[ -f "$MITHRIL_REFRESH_DIR/.mithril-refresh.json" ]]; then
    echo "$MITHRIL_REFRESH_DIR/.mithril-refresh.json"
  else
    echo "$source/.relay-1-source.json"
  fi
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
  AMARU_NETWORK="$NETWORK" cargo build --profile "$BUILD_PROFILE" --bin amaru
}

require_amaru_node_binary() {
  [[ -x "$(amaru_node_binary)" ]] || die "Amaru node binary not found: $(amaru_node_binary); run ./process-compose.sh setup first"
}

run_mithril_refresh() {
  validate_config
  if truthy "$REFRESH_FROM_MITHRIL"; then
    if ! refresh_from_mithril; then
      if validate_amaru_source_databases; then
        echo "[mithril-refresh] refresh failed; using existing refreshed databases from $MITHRIL_REFRESH_DIR"
      else
        return 1
      fi
    fi
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
  local trace_arg=""
  if truthy "$AMARU_MIDDLE_WITH_JSON_TRACES"; then
    trace_arg="--with-json-traces"
  fi
  export AMARU_WITH_OPEN_TELEMETRY="$AMARU_MIDDLE_WITH_OPEN_TELEMETRY"
  export AMARU_LOG="$AMARU_MIDDLE_LOG"
  export AMARU_TRACE="$AMARU_MIDDLE_TRACE"
  export OTEL_SERVICE_NAME="$AMARU_MIDDLE_OTEL_SERVICE_NAME"
  export OTEL_SERVICE_INSTANCE_ID="$AMARU_MIDDLE_OTEL_SERVICE_INSTANCE_ID"
  export OTEL_EXPORTER_OTLP_ENDPOINT
  export OTEL_EXPORTER_OTLP_METRICS_ENDPOINT
  export OTEL_METRIC_EXPORT_INTERVAL_MS
  ulimit -n 65536
  require_amaru_node_binary
  mark_database_dir_dirty "$RUNDIR/amaru/chain.$NETWORK.db"
  mark_database_dir_dirty "$RUNDIR/amaru/ledger.$NETWORK.db"
  "$(amaru_node_binary)" ${trace_arg:+"$trace_arg"} run \
    --peer-address "127.0.0.1:$UPSTREAM_PORT" \
    --listen-address "0.0.0.0:$LISTEN_PORT" \
    --chain-dir "$RUNDIR/amaru/chain.$NETWORK.db" \
    --ledger-dir "$RUNDIR/amaru/ledger.$NETWORK.db" \
    2>&1 | tee "$AMARU_MIDDLE_LOG_FILE"
}

run_amaru_downstream() {
  cd "$AMARU_DIR"
  local trace_arg=""
  if truthy "$AMARU_DOWNSTREAM_WITH_JSON_TRACES"; then
    trace_arg="--with-json-traces"
  fi
  export AMARU_WITH_OPEN_TELEMETRY="$AMARU_DOWNSTREAM_WITH_OPEN_TELEMETRY"
  export AMARU_LOG="$AMARU_DOWNSTREAM_LOG"
  export AMARU_TRACE="$AMARU_DOWNSTREAM_TRACE"
  export OTEL_SERVICE_NAME="$AMARU_DOWNSTREAM_OTEL_SERVICE_NAME"
  export OTEL_SERVICE_INSTANCE_ID="$AMARU_DOWNSTREAM_OTEL_SERVICE_INSTANCE_ID"
  export OTEL_EXPORTER_OTLP_ENDPOINT
  export OTEL_EXPORTER_OTLP_METRICS_ENDPOINT
  export OTEL_METRIC_EXPORT_INTERVAL_MS
  ulimit -n 65536
  require_amaru_node_binary
  mark_database_dir_dirty "$RUNDIR/amaru-downstream/chain.$NETWORK.db"
  mark_database_dir_dirty "$RUNDIR/amaru-downstream/ledger.$NETWORK.db"
  "$(amaru_node_binary)" ${trace_arg:+"$trace_arg"} run \
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
  [[ -f "$AMARU_MIDDLE_LOG_FILE" ]] || exit 1
  grep -E '("message":"listening"|listening)' "$AMARU_MIDDLE_LOG_FILE" >/dev/null 2>&1
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

      function field_value(text, field,    prefix, start, rest, stop) {
        prefix = "\"" field "\":\""
        start = index(text, prefix)
        if (start == 0) return ""
        rest = substr(text, start + length(prefix))
        stop = index(rest, "\"")
        return stop == 0 ? rest : substr(rest, 1, stop - 1)
      }

      function remember_submitted_tx(text,    tx_id) {
        tx_id = text
        sub(/^.*tx_id=/, "", tx_id)
        sub(/[^0-9a-fA-F].*$/, "", tx_id)
        if (tx_id != "") submitted_tx_ids[substr(tx_id, 1, 12)] = 1
      }

      function is_amaru_node(source) {
        return source == "amaru-middle" || source == "amaru-downstream"
      }

      BEGIN {
        source = "log"
        pending_middle_txs = 0
        pending_upstream_txs = 0
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
        marker_style = ""
        marker = ""

        if (source == "submit-tx" && lower ~ /built transaction .*tx_id=/) {
          remember_submitted_tx(line)
        }

        if (source == "amaru-middle" && lower ~ /transaction accepted into mempool/) {
          pending_middle_txs++
        } else if (source == "cardano-upstream" && line ~ /TraceMempoolAddedTx/) {
          pending_upstream_txs++
        }

        if (source == "amaru-middle" && pending_middle_txs > 0 && lower ~ /adopted tip/) {
          marker_style = "1;32"
          marker = sprintf(">>> BLOCK AFTER %d TX >>> ", pending_middle_txs)
          pending_middle_txs = 0
        } else if (source == "cardano-upstream" && pending_upstream_txs > 0 && line ~ /Chain extended, new tip:/) {
          marker_style = "1;32"
          marker = sprintf(">>> BLOCK AFTER %d TX >>> ", pending_upstream_txs)
          pending_upstream_txs = 0
        } else if (is_amaru_node(source) && lower ~ /transaction found in block/ && field_value(line, "tx_id") in submitted_tx_ids) {
          marker_style = "1;36"
          marker = ">>> TX >>> "
        } else if (is_amaru_node(source) && lower ~ /transaction invalid in block/ && field_value(line, "tx_id") in submitted_tx_ids) {
          marker_style = "1;36"
          marker = ">>> TX >>> "
        } else if (lower ~ /error|rejected|giving up|failed|non-retryable/) {
          style = "1;31"
        } else if (lower ~ /warn/) {
          style = "1;33"
        } else if (source == "submit-tx" && lower ~ /submitting|building transaction|built transaction|response: http 202|selected utxo/) {
          marker_style = "1;36"
          marker = ">>> TX >>> "
        } else if (source == "amaru-downstream" && lower ~ /transaction accepted into mempool/) {
          marker_style = "1;36"
          marker = ">>> TX >>> "
        } else if (source == "amaru-middle" && lower ~ /transaction accepted into mempool/) {
          marker_style = "1;36"
          marker = ">>> TX >>> "
        } else if (source == "cardano-upstream" && line ~ /TraceMempoolAddedTx/) {
          marker_style = "1;36"
          marker = ">>> TX >>> "
        }

        label_style = source == "cardano-upstream" ? "33" : source == "amaru-middle" ? "35" : source == "amaru-downstream" ? "32" : source == "submit-tx" ? "36" : "37"
        label = sprintf("[%-17s]", source)
        label = paint(label_style, label)
        message = marker == "" ? line : paint(marker_style, marker) line
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
  generate_submit
}

restart_submit_tx_replicas() {
  have process-compose || die "process-compose not found"
  cd "$SCRIPT_DIR"
  local process
  while IFS= read -r process; do
    case "$process" in
      6-submit-tx | 6-submit-tx-*) process-compose process restart "$process" ;;
    esac
  done < <(process-compose list)
}

run_refuel_submit_wallet() {
  refuel_submit_wallet
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

grafana_metrics_url() {
  local queries_json="$1"
  local panes
  panes="$(
    jq -cn \
      --argjson queries_json "$queries_json" \
      '{
        metrics: {
          datasource: "prometheus",
          queries: [
            $queries_json[]
            | {
              refId: .name,
              datasource: {type: "prometheus", uid: "prometheus"},
              expr: .expr,
              legendFormat: (.legend // .name),
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

grafana_mempool_dashboard_url() {
  printf '%s/d/amaru-relay-mempool/amaru-relay-mempool?orgId=1&refresh=5s&from=now-15m&to=now\n' \
    "$TELEMETRY_GRAFANA_URL"
}

telemetry_urls() {
  grafana_trace_url "traces" '{ resource.service.name = "amaru-middle" && name = "decode_header"}'
  grafana_mempool_dashboard_url
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
  submit-tx-restart-all) restart_submit_tx_replicas ;;
  refuel-submit-wallet | refuel-submit-tx) run_refuel_submit_wallet ;;
  telemetry-up) telemetry_up ;;
  telemetry-down) telemetry_down ;;
  telemetry-open | open-telemetry) open_telemetry ;;
  telemetry-urls) telemetry_urls ;;
  run)
    case "${2:-}" in
      mithril-refresh | 1-mithril-refresh) run_mithril_refresh ;;
      setup | 2-setup) setup ;;
      cardano-upstream | cardano-node | 3-cardano-node) run_cardano_upstream ;;
      amaru-middle | 4-amaru-middle) run_amaru_middle ;;
      amaru-downstream | 5-amaru-downstream) run_amaru_downstream ;;
      watch | 9-watch) run_watch ;;
      telemetry-open | telemetry | 8-telemetry) open_telemetry ;;
      submit-tx | 6-submit-tx) run_submit_tx; exit $? ;;
      refuel-submit-wallet | refuel-submit-tx | 7-refuel-submit-wallet) run_refuel_submit_wallet; exit $? ;;
      *) die "usage: $0 run {mithril-refresh|setup|cardano-upstream|amaru-middle|amaru-downstream|watch|telemetry-open|submit-tx|refuel-submit-wallet}" ;;
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
  *) die "usage: $0 {up|refresh|down|status|setup|submit-tx-restart-all|refuel-submit-wallet|run <process>|ready <process>}" ;;
esac
