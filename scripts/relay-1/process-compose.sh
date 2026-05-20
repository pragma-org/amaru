#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
AMARU_DIR="${AMARU_DIR:-$(cd "$SCRIPT_DIR/../.." && pwd)}"

LOGDIR="${LOGDIR:-/tmp/amaru-relay-1}"
RUNDIR="${RUNDIR:-$SCRIPT_DIR/run}"
NETWORK="${AMARU_NETWORK:-preprod}"
BUILD_PROFILE="${BUILD_PROFILE:-dev}"
REFRESH_FROM_MITHRIL="${REFRESH_FROM_MITHRIL:-true}"
CARDANO_NODE_INIT_FROM_MITHRIL="${CARDANO_NODE_INIT_FROM_MITHRIL:-true}"
CARDANO_NODE_RELEASE_VERSION="${CARDANO_NODE_RELEASE_VERSION:-11.0.1}"
CARDANO_NODE_DOWNLOAD_BASE_URL="${CARDANO_NODE_DOWNLOAD_BASE_URL:-https://github.com/IntersectMBO/cardano-node/releases/download/$CARDANO_NODE_RELEASE_VERSION}"
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
DEFAULT_AMARU_CHAIN_SOURCE_DIR="$MITHRIL_REFRESH_DIR/chain.$NETWORK.db"
DEFAULT_AMARU_LEDGER_SOURCE_DIR="$MITHRIL_REFRESH_DIR/ledger.$NETWORK.db"
AMARU_CHAIN_SOURCE_DIR="${AMARU_CHAIN_SOURCE_DIR:-$DEFAULT_AMARU_CHAIN_SOURCE_DIR}"
AMARU_LEDGER_SOURCE_DIR="${AMARU_LEDGER_SOURCE_DIR:-$DEFAULT_AMARU_LEDGER_SOURCE_DIR}"

default_tx_payment_skey() {
  if [[ -f "$SCRIPT_DIR/keys/$NETWORK/payment.skey" ]]; then
    echo "$SCRIPT_DIR/keys/$NETWORK/payment.skey"
  elif [[ -f "$SCRIPT_DIR/run/$NETWORK-wallet/payment.skey" ]]; then
    echo "$SCRIPT_DIR/run/$NETWORK-wallet/payment.skey"
  else
    echo "$SCRIPT_DIR/keys/payment.skey"
  fi
}

TX_PAYMENT_SKEY="${TX_PAYMENT_SKEY:-$(default_tx_payment_skey)}"
TX_WAIT_FOR_SYNC="${TX_WAIT_FOR_SYNC:-true}"
CLEAR_SUBMIT_TX_CLAIMS_ON_START="${CLEAR_SUBMIT_TX_CLAIMS_ON_START:-true}"
default_cardano_upstream_mode() {
  if [[ "$NETWORK" == "mainnet" ]]; then
    echo "public"
  else
    echo "local"
  fi
}

CARDANO_UPSTREAM_MODE="${CARDANO_UPSTREAM_MODE:-$(default_cardano_upstream_mode)}"
PUBLIC_UPSTREAM_PEER_ADDRESS="${PUBLIC_UPSTREAM_PEER_ADDRESS:-backbone.cardano.iog.io:3001}"
default_tx_query_source() {
  if [[ "$CARDANO_UPSTREAM_MODE" == "public" ]]; then
    echo "koios"
  else
    echo "local"
  fi
}

TX_QUERY_SOURCE="${TX_QUERY_SOURCE:-$(default_tx_query_source)}"
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
AMARU_CONSENSUS_TRUST_UPSTREAM_HEADERS="${AMARU_CONSENSUS_TRUST_UPSTREAM_HEADERS:-true}"
AMARU_MIDDLE_OTEL_SERVICE_NAME="${AMARU_MIDDLE_OTEL_SERVICE_NAME:-amaru-middle}"
AMARU_DOWNSTREAM_OTEL_SERVICE_NAME="${AMARU_DOWNSTREAM_OTEL_SERVICE_NAME:-amaru-downstream}"
AMARU_MIDDLE_TRACE="${AMARU_MIDDLE_TRACE:-info,amaru::consensus=trace,amaru::stores::consensus=trace,amaru::stores::ledger=trace,amaru::stores::rocksdb=trace,amaru::mempool=trace,amaru::ledger::state=trace,amaru::ledger::context=trace,amaru::ledger::governance=trace,amaru::protocols::manager=trace}"
AMARU_DOWNSTREAM_TRACE="${AMARU_DOWNSTREAM_TRACE:-info,amaru::consensus=trace,amaru::stores::consensus=trace,amaru::stores::ledger=trace,amaru::stores::rocksdb=trace,amaru::mempool=trace,amaru::ledger::state=trace,amaru::ledger::context=trace,amaru::ledger::governance=trace,amaru::protocols::manager=trace}"

if [[ -z "${CARDANO_NODE:-}" ]]; then
  CARDANO_NODE="$CARDANO_NODE_HOME/bin/cardano-node"
elif [[ "$CARDANO_NODE_HOME_WAS_SET" == false && ! -x "$CARDANO_NODE" ]]; then
  CARDANO_NODE="$CARDANO_NODE_HOME/bin/cardano-node"
fi
CARDANO_NODE_CONFIG_DIR="${CARDANO_NODE_CONFIG_DIR:-$AMARU_DIR/cardano-node-config/$NETWORK}"
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
  if ! public_cardano_upstream_enabled; then
    require_cardano_node
  fi
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

public_cardano_upstream_enabled() {
  [[ "$CARDANO_UPSTREAM_MODE" == "public" ]]
}

expected_network_magic() {
  case "$NETWORK" in
    preprod) echo 1 ;;
    preview) echo 2 ;;
    *) return 1 ;;
  esac
}

validate_network_config() {
  local expected_magic actual_magic

  expected_magic="$(expected_network_magic || true)"
  [[ -n "$expected_magic" ]] || return 0

  actual_magic="$(network_magic)"
  if [[ "$actual_magic" != "$expected_magic" ]]; then
    die "AMARU_NETWORK=$NETWORK expects testnet magic $expected_magic, but CARDANO_NODE_CONFIG_DIR=$CARDANO_NODE_CONFIG_DIR reports magic $actual_magic"
  fi
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

cardano_node_archive_platform() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"

  case "$os:$arch" in
    Darwin:arm64) echo "macos-arm64" ;;
    Darwin:x86_64) echo "macos-amd64" ;;
    Linux:aarch64 | Linux:arm64) echo "linux-arm64" ;;
    Linux:x86_64) echo "linux-amd64" ;;
    *) die "unsupported platform for cardano-node release download: $os/$arch" ;;
  esac
}

download_cardano_node_home() {
  local platform archive_name archive_path checksums_path expected actual

  platform="$(cardano_node_archive_platform)"
  archive_name="cardano-node-$CARDANO_NODE_RELEASE_VERSION-$platform.tar.gz"
  archive_path="$LOGDIR/$archive_name"
  checksums_path="$LOGDIR/cardano-node-$CARDANO_NODE_RELEASE_VERSION-sha256sums.txt"

  have curl || die "curl not found; cannot download cardano-node $CARDANO_NODE_RELEASE_VERSION"
  have tar || die "tar not found; cannot unpack cardano-node $CARDANO_NODE_RELEASE_VERSION"

  mkdir -p "$LOGDIR"
  if [[ ! -f "$archive_path" ]]; then
    echo "[setup] downloading $archive_name"
    curl -fL "$CARDANO_NODE_DOWNLOAD_BASE_URL/$archive_name" -o "$archive_path"
  else
    echo "[setup] using cached $archive_path"
  fi

  if have shasum; then
    if [[ ! -f "$checksums_path" ]]; then
      echo "[setup] downloading cardano-node checksums"
      curl -fsSL "$CARDANO_NODE_DOWNLOAD_BASE_URL/cardano-node-$CARDANO_NODE_RELEASE_VERSION-sha256sums.txt" -o "$checksums_path"
    fi
    expected="$(awk -v name="$archive_name" '$2 == name { print $1 }' "$checksums_path")"
    [[ -n "$expected" ]] || die "checksum for $archive_name not found in $checksums_path"
    actual="$(shasum -a 256 "$archive_path" | awk '{ print $1 }')"
    [[ "$actual" == "$expected" ]] || die "checksum mismatch for $archive_path"
  fi

  rm -rf "$CARDANO_NODE_HOME.in-progress"
  mkdir -p "$CARDANO_NODE_HOME.in-progress"
  tar -xzf "$archive_path" -C "$CARDANO_NODE_HOME.in-progress"
  rm -rf "$CARDANO_NODE_HOME"
  mv "$CARDANO_NODE_HOME.in-progress" "$CARDANO_NODE_HOME"
}

repair_downloaded_cardano_node_home() {
  if [[ "$(uname -s)" != Darwin || "$CARDANO_NODE_HOME_WAS_SET" == true ]]; then
    return
  fi

  have codesign || return

  if "$CARDANO_NODE" --version >/dev/null 2>&1; then
    return
  fi

  codesign --force --sign - "$CARDANO_NODE_HOME/bin/cardano-node" "$CARDANO_NODE_HOME/bin/db-analyser"
}

setup() {
  require_unscaled_process setup
  if [[ "$CARDANO_NODE_HOME_WAS_SET" == true ]]; then
    echo "[setup] CARDANO_NODE_HOME is set: $CARDANO_NODE_HOME"
  elif [[ -x "$CARDANO_NODE_HOME/bin/cardano-node" && -x "$CARDANO_NODE_HOME/bin/db-analyser" ]]; then
    echo "[setup] using existing cardano-node tools in $CARDANO_NODE_HOME"
  else
    download_cardano_node_home
  fi

  [[ -x "$CARDANO_NODE_HOME/bin/db-analyser" ]] || die "db-analyser not found at $CARDANO_NODE_HOME/bin/db-analyser"
  [[ -x "$CARDANO_NODE" ]] || die "cardano-node not found at $CARDANO_NODE"
  repair_downloaded_cardano_node_home
  echo "[setup] cardano-node tools ready in $CARDANO_NODE_HOME"
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
  echo "[initialize] validating configuration and source databases..."
  validate_amaru_source_databases
  have rsync || die "rsync not found; cannot synchronize Amaru databases"
  echo "[initialize] ensuring log and run directories exist..."
  ensure_dirs
  echo "[initialize] clearing previous relay logs from $LOGDIR..."
  rm -f "$LOGDIR"/*.log 2>/dev/null || true
  echo "[initialize] clearing previous submit transaction artifacts..."
  rm -rf "$RUNDIR"/generated/submit-tx-* "$RUNDIR/generated/submit-tx-claims" "$RUNDIR/generated/submit-tx-active" 2>/dev/null || true
  rm -f "$RUNDIR"/generated/tx-*.body "$RUNDIR"/generated/tx-*.json "$RUNDIR"/generated/tx-*.cbor 2>/dev/null || true
  rm -f "$RUNDIR/generated/utxo.json" "$RUNDIR/generated/last-response.txt" "$RUNDIR/generated/last-response.txt.status" 2>/dev/null || true
  mkdir -p "$RUNDIR/amaru" "$RUNDIR/amaru-downstream"
  sync_database_dir "middle chain" "$AMARU_CHAIN_SOURCE_DIR" "$RUNDIR/amaru/chain.$NETWORK.db"
  sync_database_dir "middle ledger" "$AMARU_LEDGER_SOURCE_DIR" "$RUNDIR/amaru/ledger.$NETWORK.db"
  sync_database_dir "downstream chain" "$AMARU_CHAIN_SOURCE_DIR" "$RUNDIR/amaru-downstream/chain.$NETWORK.db"
  sync_database_dir "downstream ledger" "$AMARU_LEDGER_SOURCE_DIR" "$RUNDIR/amaru-downstream/ledger.$NETWORK.db"
}

initialize_cardano_node_database() {
  local source_immutable="$MITHRIL_SNAPSHOTS_DIR/$NETWORK/immutable"
  local target_db="$CARDANO_NODE_CONFIG_DIR/db"
  local target_marker="$target_db/.relay-1-mithril-source.json"
  local source_marker="$MITHRIL_REFRESH_DIR/.mithril-refresh.json"

  truthy "$CARDANO_NODE_INIT_FROM_MITHRIL" || return 0
  public_cardano_upstream_enabled && return 0
  [[ -d "$source_immutable" ]] || die "Mithril immutable files not found: $source_immutable"
  [[ -f "$source_marker" ]] || die "Mithril refresh metadata not found: $source_marker"
  have rsync || die "rsync not found; cannot initialize cardano-node database from Mithril"

  if same_mithril_snapshot_metadata "$source_marker" "$target_marker"; then
    echo "[initialize] cardano-node database already initialized from latest Mithril snapshot; skipping sync"
    return 0
  fi

  echo "[initialize] initializing cardano-node database from Mithril immutable files..."
  mkdir -p "$target_db"
  rm -rf "$target_db/ledger" "$target_db/volatile"
  mkdir -p "$target_db/immutable"
  rsync -a --delete "$source_immutable"/ "$target_db/immutable"/
  cp "$source_marker" "$target_marker"
  echo "[initialize] cardano-node immutable database initialized from $source_immutable"
  echo "[initialize] cardano-node will rebuild ledger and volatile state on next start"
}

same_mithril_snapshot_metadata() {
  local source="$1" target="$2"
  [[ -f "$source" && -f "$target" ]] || return 1
  jq -e -s '.[0].network == .[1].network and .[0].snapshot.hash == .[1].snapshot.hash' "$source" "$target" >/dev/null
}

sync_database_dir() {
  local label="$1" source="$2" destination="$3" source_marker destination_marker
  source_marker="$(database_source_marker_file "$source")"
  destination_marker="$destination/.relay-1-source.json"
  if [[ -f "$source_marker" && -f "$destination_marker" ]] && cmp -s "$source_marker" "$destination_marker"; then
    echo "[initialize] $label database unchanged; skipping sync"
    return 0
  fi

  echo "[initialize] synchronizing $label database: $source -> $destination"
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
  mkdir -p "$(dirname "$MITHRIL_REFRESH_LOG_FILE")"
  AMARU_NETWORK="$NETWORK" \
    BUILD_PROFILE="$BUILD_PROFILE" \
    CARDANO_NODE_HOME="$CARDANO_NODE_HOME" \
    STAGING_DIR="$MITHRIL_REFRESH_DIR" \
    AMARU_MITHRIL_SNAPSHOTS_DIR="$MITHRIL_SNAPSHOTS_DIR" \
    INSTALL=false \
    FORCE_REFRESH="${FORCE_REFRESH:-false}" \
    BOOTSTRAP_FROM_LATEST_MITHRIL="${BOOTSTRAP_FROM_LATEST_MITHRIL:-true}" \
    ./scripts/refresh-from-mithril \
    2>&1 | tee "$MITHRIL_REFRESH_LOG_FILE"
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
  echo "[initialize] building Amaru node binary with BUILD_PROFILE=$BUILD_PROFILE..."
  AMARU_NETWORK="$NETWORK" cargo build --profile "$BUILD_PROFILE" --bin amaru
}

require_amaru_node_binary() {
  [[ -x "$(amaru_node_binary)" ]] || die "Amaru node binary not found: $(amaru_node_binary); run ./process-compose.sh initialize first"
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
  if public_cardano_upstream_enabled; then
    die "cardano-upstream is disabled because CARDANO_UPSTREAM_MODE=public"
  fi
  require_cardano_node
  validate_network_config
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

amaru_middle_peer_address() {
  if public_cardano_upstream_enabled; then
    echo "$PUBLIC_UPSTREAM_PEER_ADDRESS"
  else
    echo "127.0.0.1:$UPSTREAM_PORT"
  fi
}

run_amaru_middle() {
  cd "$AMARU_DIR"
  validate_network_config
  local trace_arg=""
  if truthy "$AMARU_MIDDLE_WITH_JSON_TRACES"; then
    trace_arg="--with-json-traces"
  fi
  export AMARU_WITH_OPEN_TELEMETRY="$AMARU_MIDDLE_WITH_OPEN_TELEMETRY"
  export AMARU_LOG="$AMARU_MIDDLE_LOG"
  export AMARU_TRACE="$AMARU_MIDDLE_TRACE"
  export AMARU_CONSENSUS_TRUST_UPSTREAM_HEADERS
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
    --peer-address "$(amaru_middle_peer_address)" \
    --listen-address "0.0.0.0:$LISTEN_PORT" \
    --chain-dir "$RUNDIR/amaru/chain.$NETWORK.db" \
    --ledger-dir "$RUNDIR/amaru/ledger.$NETWORK.db" \
    2>&1 | tee "$AMARU_MIDDLE_LOG_FILE"
}

run_amaru_downstream() {
  cd "$AMARU_DIR"
  validate_network_config
  local trace_arg=""
  if truthy "$AMARU_DOWNSTREAM_WITH_JSON_TRACES"; then
    trace_arg="--with-json-traces"
  fi
  export AMARU_WITH_OPEN_TELEMETRY="$AMARU_DOWNSTREAM_WITH_OPEN_TELEMETRY"
  export AMARU_LOG="$AMARU_DOWNSTREAM_LOG"
  export AMARU_TRACE="$AMARU_DOWNSTREAM_TRACE"
  export AMARU_CONSENSUS_TRUST_UPSTREAM_HEADERS
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
  validate_network_config
  generate_submit
}

restart_submit_tx_replicas() {
  have process-compose || die "process-compose not found"
  cd "$SCRIPT_DIR"
  local process
  while IFS= read -r process; do
    case "$process" in
      7-submit-tx | 7-submit-tx-*) process-compose process restart "$process" ;;
    esac
  done < <(process-compose list)
}

run_refuel_submit_wallet() {
  validate_network_config
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
  [[ -n "$CARDANO_NODE_CONFIG_DIR" ]] || die "CARDANO_NODE_CONFIG_DIR must be set (directory with config.json, topology.json, etc.)"
  [[ -d "$CARDANO_NODE_CONFIG_DIR" ]] || die "CARDANO_NODE_CONFIG_DIR does not exist: $CARDANO_NODE_CONFIG_DIR"
  [[ -f "$(cardano_node_config_file)" ]] || die "cardano-node config file not found: $(cardano_node_config_file)"
  if ! public_cardano_upstream_enabled; then
    [[ -f "$(cardano_node_topology_file)" ]] || die "cardano-node topology file not found: $(cardano_node_topology_file)"
  fi
  [[ -d "$AMARU_DIR" ]] || die "AMARU_DIR does not exist: $AMARU_DIR"
  validate_network_config
  require_configured_tx
  if ! truthy "$REFRESH_FROM_MITHRIL"; then
    validate_amaru_source_databases
  fi
}

process_compose_file() {
  if ! public_cardano_upstream_enabled; then
    echo "$SCRIPT_DIR/process-compose.yaml"
    return 0
  fi

  local generated="$RUNDIR/generated/process-compose.public-upstream.yaml"
  mkdir -p "$(dirname "$generated")"
  awk '
    /^  3-cardano-node:/ { skip_process = 1; next }
    /^  6-refuel-submit-wallet:/ { skip_process = 1; next }
    skip_process && /^  [0-9][^[:space:]]*:/ { skip_process = 0 }
    skip_process { next }
    /^      3-cardano-node:/ { skip_dependency = 1; next }
    /^      6-refuel-submit-wallet:/ { skip_dependency = 1; next }
    skip_dependency && /^        / { next }
    { skip_dependency = 0; print }
  ' "$SCRIPT_DIR/process-compose.yaml" > "$generated"
  echo "$generated"
}

up() {
  have process-compose || die "process-compose not found"
  validate_up
  telemetry_up
  cd "$SCRIPT_DIR"
  exec process-compose -f "$(process_compose_file)" up
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
  initialize) initialize ;;
  submit-tx-restart-all) restart_submit_tx_replicas ;;
  refuel-submit-wallet | refuel-submit-tx) run_refuel_submit_wallet ;;
  telemetry-up) telemetry_up ;;
  telemetry-down) telemetry_down ;;
  telemetry-open | open-telemetry) open_telemetry ;;
  telemetry-urls) telemetry_urls ;;
  run)
    case "${2:-}" in
      setup | 0-setup) setup ;;
      mithril-refresh | 1-mithril-refresh) run_mithril_refresh ;;
      initialize | 2-initialize | 2-setup) initialize ;;
      cardano-upstream | cardano-node | 3-cardano-node) run_cardano_upstream ;;
      amaru-middle | 4-amaru-middle) run_amaru_middle ;;
      amaru-downstream | 5-amaru-downstream) run_amaru_downstream ;;
      watch | 9-watch) run_watch ;;
      telemetry-open | telemetry | 8-telemetry) open_telemetry ;;
      refuel-submit-wallet | refuel-submit-tx | 6-refuel-submit-wallet) run_refuel_submit_wallet; exit $? ;;
      submit-tx | 7-submit-tx) run_submit_tx; exit $? ;;
      *) die "usage: $0 run {setup|mithril-refresh|initialize|cardano-upstream|amaru-middle|amaru-downstream|watch|telemetry-open|submit-tx|refuel-submit-wallet}" ;;
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
  *) die "usage: $0 {up|refresh|down|status|setup|initialize|submit-tx-restart-all|refuel-submit-wallet|run <process>|ready <process>}" ;;
esac
