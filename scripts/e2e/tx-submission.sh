#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
AMARU_DIR="${AMARU_DIR:-$(cd "$SCRIPT_DIR/../.." && pwd)}"
COMMON_DIR="$AMARU_DIR/scripts/demos/common"

NETWORK="${AMARU_NETWORK:-preprod}"
BUILD_PROFILE="${BUILD_PROFILE:-dev}"
RUNDIR="${E2E_TX_WORK_DIR:-$AMARU_DIR/scripts/demos/relay-1/run/e2e-tx-submission}"
LOGDIR="${E2E_TX_LOG_DIR:-$RUNDIR/logs}"
RESULTS_DIR="${E2E_TX_RESULTS_DIR:-$RUNDIR/results}"
RUN_ID="${E2E_TX_RUN_ID:-$(date -u +%Y%m%dT%H%M%SZ)-$$}"
PRIVATE_DIR="$RUNDIR/private/$RUN_ID"
RESULT_DIR="$RESULTS_DIR/$RUN_ID"

[[ "$RUNDIR" != / ]] || { echo "error: unsafe E2E_TX_WORK_DIR: $RUNDIR" >&2; exit 1; }
[[ "$RUN_ID" =~ ^[A-Za-z0-9._-]+$ ]] || { echo "error: E2E_TX_RUN_ID contains unsafe characters: $RUN_ID" >&2; exit 1; }

E2E_COLOR_RESET=""
E2E_COLOR_SETUP=""
E2E_COLOR_INFO=""
E2E_COLOR_SUCCESS=""
E2E_COLOR_WARNING=""
E2E_COLOR_ERROR=""
if [[ -t 1 && -z "${NO_COLOR:-}" && "${TERM:-}" != dumb ]]; then
  E2E_COLOR_RESET=$'\033[0m'
  E2E_COLOR_SETUP=$'\033[36m'
  E2E_COLOR_INFO=$'\033[34m'
  E2E_COLOR_SUCCESS=$'\033[1;32m'
  E2E_COLOR_WARNING=$'\033[33m'
  E2E_COLOR_ERROR=$'\033[1;31m'
fi

setup_log() { printf '%s[setup]%s %s\n' "$E2E_COLOR_SETUP" "$E2E_COLOR_RESET" "$*"; }
snapshot_log() { printf '%s[snapshot]%s %s\n' "$E2E_COLOR_SETUP" "$E2E_COLOR_RESET" "$*"; }
e2e_log() { printf '%s[e2e]%s %s\n' "$E2E_COLOR_INFO" "$E2E_COLOR_RESET" "$*"; }
e2e_success() { printf '%s[e2e] %s%s\n' "$E2E_COLOR_SUCCESS" "$*" "$E2E_COLOR_RESET"; }
e2e_warning() { printf '%s[e2e] %s%s\n' "$E2E_COLOR_WARNING" "$*" "$E2E_COLOR_RESET"; }
self_test_success() { printf '%s[self-test] %s%s\n' "$E2E_COLOR_SUCCESS" "$*" "$E2E_COLOR_RESET"; }

AMARU_CHAIN_DIR="${AMARU_CHAIN_DIR:-$RUNDIR/amaru/chain.$NETWORK.db}"
AMARU_LEDGER_DIR="${AMARU_LEDGER_DIR:-$RUNDIR/amaru/ledger.$NETWORK.db}"
AMARU_LOG_FILE="${AMARU_LOG_FILE:-$LOGDIR/amaru.log}"
AMARU_LISTEN_ADDRESS="${AMARU_LISTEN_ADDRESS:-127.0.0.1:4001}"
AMARU_SUBMIT_API_ADDRESS="${AMARU_SUBMIT_API_ADDRESS:-127.0.0.1:8090}"
AMARU_PEER_ADDRESS="${AMARU_PEER_ADDRESS:-127.0.0.1:3001}"
AMARU_UPSTREAM_PEERS="${AMARU_UPSTREAM_PEERS:-1}"
AMARU_MANAGED="${E2E_TX_MANAGE_AMARU:-true}"

CARDANO_NODE_RELEASE_VERSION="${CARDANO_NODE_RELEASE_VERSION:-11.0.1}"
CARDANO_NODE_HOME_WAS_SET=false
if [[ -n "${CARDANO_NODE_HOME:-}" ]]; then
  CARDANO_NODE_HOME_WAS_SET=true
else
  CARDANO_NODE_HOME="$RUNDIR/tools/cardano-node-$CARDANO_NODE_RELEASE_VERSION"
fi
CARDANO_NODE="${CARDANO_NODE:-$CARDANO_NODE_HOME/bin/cardano-node}"
CARDANO_NODE_CONFIG_DIR="${CARDANO_NODE_CONFIG_DIR:-$AMARU_DIR/cardano-node-config/$NETWORK}"
CARDANO_NODE_DB="${CARDANO_NODE_DB:-$CARDANO_NODE_CONFIG_DIR/db}"
CARDANO_NODE_SOCKET_FILE="${CARDANO_NODE_SOCKET_FILE:-/tmp/amaru-e2e-${UID:-0}-${CARDANO_NODE_PORT:-3001}.socket}"
CARDANO_NODE_LOG_FILE="${CARDANO_NODE_LOG_FILE:-$LOGDIR/cardano-node.log}"
CARDANO_NODE_MANAGED="${E2E_TX_MANAGE_CARDANO_NODE:-true}"
CARDANO_NODE_SYNC_PROGRESS="${CARDANO_NODE_SYNC_PROGRESS:-99.9}"
CARDANO_NODE_SYNC_TIMEOUT_SECONDS="${CARDANO_NODE_SYNC_TIMEOUT_SECONDS:-14400}"
CARDANO_NODE_SOCKET_TIMEOUT_SECONDS="${CARDANO_NODE_SOCKET_TIMEOUT_SECONDS:-1800}"
CARDANO_NODE_QUERY_TIMEOUT_SECONDS="${CARDANO_NODE_QUERY_TIMEOUT_SECONDS:-1800}"
CARDANO_UPSTREAM_MODE=local
UPSTREAM_PORT="${CARDANO_NODE_PORT:-3001}"

MITHRIL_INSTALLER_COMMIT="${MITHRIL_INSTALLER_COMMIT:-791dca3c035452ae35a0361303c6e674aacf617c}"
MITHRIL_CLIENT_DISTRIBUTION="${MITHRIL_CLIENT_DISTRIBUTION:-2630.0}"
MITHRIL_CLIENT_HOME="${MITHRIL_CLIENT_HOME:-$RUNDIR/tools/mithril-client}"
MITHRIL_CLIENT="${MITHRIL_CLIENT:-$MITHRIL_CLIENT_HOME/mithril-client}"

CARDANO_CLI_RELEASE_VERSION="${CARDANO_CLI_RELEASE_VERSION:-11.0.0.0}"
CARDANO_CLI_HOME="${CARDANO_CLI_HOME:-$RUNDIR/tools/cardano-cli-$CARDANO_CLI_RELEASE_VERSION}"
CARDANO_CLI="${CARDANO_CLI:-$CARDANO_CLI_HOME/bin/cardano-cli}"

TX_PAYMENT_SKEY_WAS_SET=false
if [[ -n "${TX_PAYMENT_SKEY:-}" ]]; then
  TX_PAYMENT_SKEY_WAS_SET=true
fi
TX_WALLET_DIR="${TX_WALLET_DIR:-$RUNDIR/wallet/$NETWORK}"
TX_WALLET_SKEY="${TX_WALLET_SKEY:-$TX_WALLET_DIR/payment.skey}"
TX_WALLET_VKEY="${TX_WALLET_VKEY:-$TX_WALLET_DIR/payment.vkey}"
TX_WALLET_ADDRESS_FILE="${TX_WALLET_ADDRESS_FILE:-$TX_WALLET_DIR/payment.addr}"
TX_PAYMENT_SKEY="${TX_PAYMENT_SKEY:-$TX_WALLET_SKEY}"
TX_SUBMIT_API_ADDRESS="$AMARU_SUBMIT_API_ADDRESS"
TX_QUERY_SOURCE=local
TX_METADATA_MESSAGE="${TX_METADATA_MESSAGE:-amaru e2e $RUN_ID}"
TX_SYNC_TIMEOUT_SECONDS="${TX_SYNC_TIMEOUT_SECONDS:-3600}"
TX_SYNC_POLL_INTERVAL_SECONDS="${TX_SYNC_POLL_INTERVAL_SECONDS:-15}"
TX_SUBMIT_RETRY_LIMIT="${TX_SUBMIT_RETRY_LIMIT:-20}"
TX_SUBMIT_RETRY_DELAY="${TX_SUBMIT_RETRY_DELAY:-5}"
TX_INPUT_TIMEOUT_SECONDS="${TX_INPUT_TIMEOUT_SECONDS:-900}"
TX_INPUT_POLL_INTERVAL_SECONDS="${TX_INPUT_POLL_INTERVAL_SECONDS:-2}"
TX_MEMPOOL_TIMEOUT_SECONDS="${TX_MEMPOOL_TIMEOUT_SECONDS:-60}"
TX_MEMPOOL_POLL_INTERVAL_SECONDS="${TX_MEMPOOL_POLL_INTERVAL_SECONDS:-1}"

. "$COMMON_DIR/common.sh"
. "$COMMON_DIR/cardano-cli.sh"
. "$COMMON_DIR/cardano-node.sh"
. "$COMMON_DIR/amaru.sh"
. "$COMMON_DIR/tx.sh"

E2E_FAILURE_MESSAGE=""

die() {
  E2E_FAILURE_MESSAGE="$*"
  printf '%serror:%s %s\n' "$E2E_COLOR_ERROR" "$E2E_COLOR_RESET" "$*" >&2
  exit 1
}

AMARU_PID=""
CARDANO_NODE_PID=""
TX_PAYMENT_SKEY_INSTALLED=false

usage() {
  cat <<'EOF'
Usage: scripts/e2e/tx-submission.sh <wallet|snapshot|prepare|setup|run|self-test>

  wallet     Create the dedicated development payment key and print its faucet address.
  snapshot   Install mithril-client and download a cardano-node database snapshot.
  prepare    Run setup and snapshot for a fast first development run.
  setup      Download pinned tools/config, create the wallet, build Amaru, and bootstrap its databases.
  run        Start the topology, submit one transaction, and verify cardano-node's mempool.
  self-test  Test the strict response parsers without starting either node.

The managed development topology uses CARDANO_NODE_DB (default:
cardano-node-config/<network>/db). An existing synchronized database makes startup fast;
otherwise cardano-node initializes and synchronizes one. CI can start cardano-node itself
and set E2E_TX_MANAGE_CARDANO_NODE=false. Set E2E_TX_MANAGE_AMARU=false when
the workflow already started Amaru with its Submit API enabled.
EOF
}

target_profile_dir() {
  case "$BUILD_PROFILE" in
    dev | test) echo debug ;;
    release | bench) echo release ;;
    *) echo "$BUILD_PROFILE" ;;
  esac
}

amaru_binary() {
  local target_dir="${CARGO_TARGET_DIR:-$AMARU_DIR/target}"
  if [[ -n "${CARGO_BUILD_TARGET:-}" ]]; then
    target_dir="$target_dir/$CARGO_BUILD_TARGET"
  fi
  echo "${AMARU_NODE_BINARY:-$target_dir/$(target_profile_dir)/amaru}"
}

require_base_tools() {
  local tool missing=()
  for tool in jq curl xxd awk sort tail date tar; do
    have "$tool" || missing+=("$tool")
  done
  [[ ${#missing[@]} -eq 0 ]] || die "required tools are missing: ${missing[*]}"
}

ensure_amaru_binary() {
  if [[ -n "${AMARU_NODE_BINARY:-}" ]]; then
    [[ -x "$AMARU_NODE_BINARY" ]] || die "AMARU_NODE_BINARY is not executable: $AMARU_NODE_BINARY"
    return
  fi
  have cargo || die "cargo not found"
  setup_log "building the current Amaru source with BUILD_PROFILE=$BUILD_PROFILE"
  (cd "$AMARU_DIR" && cargo build --locked --profile "$BUILD_PROFILE" --bin amaru)
}

amaru_databases_ready() {
  [[ -d "$AMARU_CHAIN_DIR" && -d "$AMARU_LEDGER_DIR" ]]
}

ensure_amaru_databases() {
  if amaru_databases_ready; then
    setup_log "using Amaru databases $AMARU_CHAIN_DIR and $AMARU_LEDGER_DIR"
    return
  fi
  if [[ -e "$AMARU_CHAIN_DIR" || -e "$AMARU_LEDGER_DIR" ]]; then
    die "only one Amaru database exists; provide a matching AMARU_CHAIN_DIR and AMARU_LEDGER_DIR or remove the incomplete E2E database"
  fi
  setup_log "bootstrapping Amaru databases for $NETWORK"
  "$(amaru_binary)" node bootstrap \
    --network "$NETWORK" \
    --chain-dir "$AMARU_CHAIN_DIR" \
    --ledger-dir "$AMARU_LEDGER_DIR"
}

ensure_cardano_node_tools() {
  if ! truthy "$CARDANO_NODE_MANAGED"; then
    setup_log "cardano-node is externally managed; skipping its release download"
    return
  fi
  if [[ "$CARDANO_NODE_HOME_WAS_SET" == true ]]; then
    require_cardano_node
  elif [[ ! -x "$CARDANO_NODE" ]]; then
    download_cardano_node_home
  fi
  require_cardano_node
  repair_downloaded_cardano_node_home
}

ensure_payment_wallet() {
  local address address_tmp
  local -a network_args=()

  if [[ "$TX_PAYMENT_SKEY_WAS_SET" == true ]]; then
    setup_log "using configured transaction signing key"
    return
  fi

  mkdir -p "$TX_WALLET_DIR"
  if [[ ! -f "$TX_WALLET_SKEY" ]]; then
    [[ ! -e "$TX_WALLET_VKEY" ]] ||
      die "wallet verification key exists without its signing key: $TX_WALLET_VKEY"
    setup_log "creating dedicated $NETWORK E2E payment key in $TX_WALLET_DIR"
    (umask 077 && "$CARDANO_CLI" conway address key-gen \
      --verification-key-file "$TX_WALLET_VKEY" \
      --signing-key-file "$TX_WALLET_SKEY")
  elif [[ ! -f "$TX_WALLET_VKEY" ]]; then
    "$CARDANO_CLI" conway key verification-key \
      --signing-key-file "$TX_WALLET_SKEY" \
      --verification-key-file "$TX_WALLET_VKEY"
  fi
  chmod 600 "$TX_WALLET_SKEY"

  while IFS= read -r arg; do
    network_args+=("$arg")
  done < <(cardano_cli_network_args)
  address="$("$CARDANO_CLI" conway address build \
    --payment-verification-key-file "$TX_WALLET_VKEY" \
    "${network_args[@]}")"
  address_tmp="$TX_WALLET_ADDRESS_FILE.tmp.$$"
  printf '%s\n' "$address" >"$address_tmp"
  mv "$address_tmp" "$TX_WALLET_ADDRESS_FILE"

  setup_log "E2E payment address: $address"
  setup_log "fund it on $NETWORK before running the test: https://docs.cardano.org/cardano-testnets/tools/faucet/"
  setup_log "signing key: $TX_WALLET_SKEY"
}

runner_wallet() {
  require_base_tools
  mkdir -p "$RUNDIR" "$LOGDIR"
  download_official_cardano_node_config
  validate_network_config
  ensure_cardano_cli
  require_cardano_cli
  ensure_payment_wallet
  validate_configured_tx_inputs
}

mithril_network_name() {
  case "$NETWORK" in
    preprod) echo release-preprod ;;
    *) die "automatic cardano-node snapshot download is currently supported only for preprod, not $NETWORK" ;;
  esac
}

ensure_mithril_client() {
  local installer="$LOGDIR/mithril-install-$MITHRIL_INSTALLER_COMMIT.sh"

  if [[ -x "$MITHRIL_CLIENT" ]]; then
    snapshot_log "using mithril-client at $MITHRIL_CLIENT"
    return
  fi
  mkdir -p "$MITHRIL_CLIENT_HOME" "$LOGDIR"
  if [[ ! -f "$installer" ]]; then
    snapshot_log "downloading the pinned official Mithril installer"
    curl -fsSL \
      "https://raw.githubusercontent.com/IntersectMBO/mithril/$MITHRIL_INSTALLER_COMMIT/mithril-install.sh" \
      -o "$installer"
  fi
  sh "$installer" \
    -c mithril-client \
    -d "$MITHRIL_CLIENT_DISTRIBUTION" \
    -p "$MITHRIL_CLIENT_HOME"
  [[ -x "$MITHRIL_CLIENT" ]] || die "Mithril installer did not create $MITHRIL_CLIENT"
}

runner_snapshot() {
  local mithril_network genesis_verification_key ancillary_verification_key config_base download_dir

  require_base_tools
  if [[ -d "$CARDANO_NODE_DB/immutable" ]]; then
    snapshot_log "using existing cardano-node database $CARDANO_NODE_DB"
    return
  fi

  if ! truthy "$CARDANO_NODE_MANAGED"; then
    die "cardano-node is externally managed; its database snapshot must be managed externally too"
  fi

  mithril_network="$(mithril_network_name)"
  config_base="https://raw.githubusercontent.com/IntersectMBO/mithril/$MITHRIL_INSTALLER_COMMIT/mithril-infra/configuration/$mithril_network"
  ensure_mithril_client
  snapshot_log "downloading Mithril verification keys for $mithril_network"
  genesis_verification_key="$(curl -fsSL "$config_base/genesis.vkey")"
  ancillary_verification_key="$(curl -fsSL "$config_base/ancillary.vkey")"
  [[ -n "$genesis_verification_key" ]] || die "empty Mithril genesis verification key"
  [[ -n "$ancillary_verification_key" ]] || die "empty Mithril ancillary verification key"

  [[ "$(basename "$CARDANO_NODE_DB")" == db ]] ||
    die "Mithril creates a db subdirectory; CARDANO_NODE_DB must end in /db: $CARDANO_NODE_DB"
  download_dir="$(dirname "$CARDANO_NODE_DB")"
  mkdir -p "$download_dir"
  if [[ -d "$CARDANO_NODE_DB" ]]; then
    rmdir "$CARDANO_NODE_DB" 2>/dev/null ||
      die "incomplete cardano-node database exists at $CARDANO_NODE_DB; move it aside before retrying"
  elif [[ -e "$CARDANO_NODE_DB" ]]; then
    die "CARDANO_NODE_DB exists and is not a directory: $CARDANO_NODE_DB"
  fi
  snapshot_log "downloading the latest $NETWORK cardano-node database to $CARDANO_NODE_DB"
  AGGREGATOR_ENDPOINT="https://aggregator.$mithril_network.api.mithril.network/aggregator" \
    GENESIS_VERIFICATION_KEY="$genesis_verification_key" \
    "$MITHRIL_CLIENT" cardano-db download \
      --download-dir "$download_dir" \
      --include-ancillary \
      --ancillary-verification-key "$ancillary_verification_key" \
      latest
  [[ -d "$CARDANO_NODE_DB/immutable" ]] ||
    die "Mithril download completed without creating $CARDANO_NODE_DB/immutable"
  snapshot_log "cardano-node database is ready"
}

runner_setup() {
  runner_wallet
  mkdir -p "$RESULTS_DIR"
  ensure_cardano_node_tools
  ensure_amaru_binary
  ensure_amaru_databases
  setup_log "transaction submission E2E prerequisites are ready"
}

runner_prepare() {
  runner_setup
  runner_snapshot
  setup_log "fast transaction submission E2E environment is ready"
}

install_base64_payment_key() {
  if [[ "${TX_PAYMENT_SKEY_BASE64+x}" != x ]]; then
    return
  fi
  [[ -n "$TX_PAYMENT_SKEY_BASE64" ]] || die "TX_PAYMENT_SKEY_BASE64 is empty"
  have base64 || die "base64 is required to install TX_PAYMENT_SKEY_BASE64"
  mkdir -p "$(dirname "$TX_PAYMENT_SKEY")"
  if ! (umask 077 && printf '%s' "$TX_PAYMENT_SKEY_BASE64" | base64 --decode >"$TX_PAYMENT_SKEY"); then
    rm -f "$TX_PAYMENT_SKEY"
    die "TX_PAYMENT_SKEY_BASE64 is not valid base64"
  fi
  if [[ ! -s "$TX_PAYMENT_SKEY" ]]; then
    rm -f "$TX_PAYMENT_SKEY"
    die "TX_PAYMENT_SKEY_BASE64 decoded to an empty key"
  fi
  TX_PAYMENT_SKEY_INSTALLED=true
  unset TX_PAYMENT_SKEY_BASE64
}

prepare_cardano_database() {
  if [[ -d "$CARDANO_NODE_DB/immutable" ]]; then
    setup_log "using cardano-node database $CARDANO_NODE_DB"
    return
  fi
  if ! truthy "$CARDANO_NODE_MANAGED"; then
    die "synchronized external cardano-node database not found at $CARDANO_NODE_DB"
  fi
  mkdir -p "$CARDANO_NODE_DB"
  e2e_warning "no cardano-node snapshot found at $CARDANO_NODE_DB; cardano-node will initialize and synchronize it"
  e2e_warning "set CARDANO_NODE_DB to an existing synchronized database for a faster first run"
}

prepare_cardano_node_config_file() {
  local config generated
  config="$(cardano_node_config_file)"
  CARDANO_NODE_EFFECTIVE_CONFIG_FILE="$config"
  [[ "$(uname -s)" == Darwin ]] || return 0

  mkdir -p "$RUNDIR/generated"
  generated="$RUNDIR/generated/cardano-config.json"
  jq '
    .TraceOptionResourceFrequency = 0
    | .TraceOptions[""].backends = (
        (.TraceOptions[""].backends // [])
        | map(select(startswith("PrometheusSimple") | not))
      )
  ' "$config" >"$generated"
  CARDANO_NODE_EFFECTIVE_CONFIG_FILE="$generated"
  e2e_log "disabled resource metrics for the managed macOS cardano-node"
}

cardano_node_effective_config_file() {
  echo "${CARDANO_NODE_EFFECTIVE_CONFIG_FILE:-$(cardano_node_config_file)}"
}

start_cardano_node() {
  if ! truthy "$CARDANO_NODE_MANAGED"; then
    e2e_log "using externally managed cardano-node socket $CARDANO_NODE_SOCKET_FILE"
    return
  fi
  require_cardano_node
  validate_network_config
  prepare_cardano_node_config_file
  prepare_cardano_node_topology_file
  ((${#CARDANO_NODE_SOCKET_FILE} <= 100)) ||
    die "cardano-node socket path is too long (${#CARDANO_NODE_SOCKET_FILE} bytes, maximum supported is 100): $CARDANO_NODE_SOCKET_FILE"
  mkdir -p "$(dirname "$CARDANO_NODE_SOCKET_FILE")" "$LOGDIR"
  rm -f "$CARDANO_NODE_SOCKET_FILE"
  e2e_log "starting cardano-node on 127.0.0.1:$UPSTREAM_PORT"
  "$CARDANO_NODE" run \
    --config "$(cardano_node_effective_config_file)" \
    --topology "$(cardano_node_effective_topology_file)" \
    --database-path "$(cardano_node_database_dir)" \
    --socket-path "$CARDANO_NODE_SOCKET_FILE" \
    --port "$UPSTREAM_PORT" \
    >"$CARDANO_NODE_LOG_FILE" 2>&1 &
  CARDANO_NODE_PID=$!
}

wait_for_cardano_node() {
  wait_for_cardano_socket
  wait_for_cardano_query
  wait_for_cardano_sync_progress "$CARDANO_NODE_SYNC_PROGRESS" "$CARDANO_NODE_SYNC_TIMEOUT_SECONDS"
  e2e_log "cardano-node is ready at slot $(cardano_node_tip_slot)"
}

start_amaru() {
  if ! truthy "$AMARU_MANAGED"; then
    e2e_log "using externally managed Amaru Submit API at $AMARU_SUBMIT_API_ADDRESS"
    return
  fi
  mkdir -p "$LOGDIR"
  : >"$AMARU_LOG_FILE"
  e2e_log "starting Amaru from current source; upstream=$AMARU_PEER_ADDRESS submit_api=$AMARU_SUBMIT_API_ADDRESS"
  AMARU_WITH_OPEN_TELEMETRY=false \
    AMARU_COLOR=never \
    AMARU_LOG="${AMARU_LOG:-info}" \
    AMARU_TRACE="${AMARU_TRACE:-info}" \
    "$(amaru_binary)" node run \
      --migrate-chain-db \
      --no-tui \
      --network "$NETWORK" \
      --peer-address "$AMARU_PEER_ADDRESS" \
      --upstream-peers "$AMARU_UPSTREAM_PEERS" \
      --listen-address "$AMARU_LISTEN_ADDRESS" \
      --submit-api-address "$AMARU_SUBMIT_API_ADDRESS" \
      --chain-dir "$AMARU_CHAIN_DIR" \
      --ledger-dir "$AMARU_LEDGER_DIR" \
      >"$AMARU_LOG_FILE" 2>&1 &
  AMARU_PID=$!
}

wait_for_amaru_submit_api() {
  local timeout="${AMARU_SUBMIT_API_TIMEOUT_SECONDS:-300}" elapsed
  for ((elapsed = 0; elapsed < timeout; elapsed++)); do
    if curl --max-time 2 -s -o /dev/null "http://$AMARU_SUBMIT_API_ADDRESS/"; then
      e2e_log "Amaru Submit API is ready"
      return
    fi
    if [[ -n "$AMARU_PID" ]] && ! kill -0 "$AMARU_PID" 2>/dev/null; then
      die "Amaru stopped before its Submit API became ready; see $AMARU_LOG_FILE"
    fi
    sleep 1
  done
  die "Amaru Submit API did not become ready within ${timeout}s; see $AMARU_LOG_FILE"
}

select_transaction_input() {
  local utxo_file="$1"
  jq -er --argjson minimum "$((TX_OUTPUT_LOVELACE + TX_FEE_BUFFER_LOVELACE))" '
    [
      to_entries[]
      | select(((.value.value | keys) - ["lovelace"] | length) == 0)
      | {tx_in: .key, lovelace: (.value.value.lovelace // 0)}
      | select(.lovelace >= $minimum)
    ]
    | sort_by(.lovelace)
    | first
    | select(. != null)
    | [.tx_in, .lovelace]
    | @tsv
  ' "$utxo_file"
}

wait_for_transaction_input() {
  local socket="$1" address="$2" utxo_file="$3"
  local timeout="$TX_INPUT_TIMEOUT_SECONDS" interval="$TX_INPUT_POLL_INTERVAL_SECONDS" elapsed record

  for ((elapsed = 0; elapsed < timeout; elapsed += interval)); do
    if query_payment_utxo "$socket" "$address" "$utxo_file" && record="$(select_transaction_input "$utxo_file")"; then
      SELECTED_TX_RECORD="$record"
      return
    fi
    if ((elapsed % 30 == 0)); then
      e2e_warning "waiting for a spendable UTxO at $address (${elapsed}s/${timeout}s)"
    fi
    managed_cardano_node_stopped && cardano_node_stopped_error "while waiting for the payment UTxO"
    sleep "$interval"
  done
  die "no pure-ADA UTxO covering the minimum output and fee became visible at $address within ${timeout}s"
}

wait_for_cardano_mempool() {
  local tx_id="$1" response_file="$2" timeout="$TX_MEMPOOL_TIMEOUT_SECONDS" elapsed state
  for ((elapsed = 0; elapsed < timeout; elapsed += TX_MEMPOOL_POLL_INTERVAL_SECONDS)); do
    state="$(cardano_node_mempool_tx_state "$tx_id" "$response_file")" ||
      die "cardano-node returned an invalid tx-mempool response for tx_id=$tx_id"
    if [[ "$state" == present ]]; then
      e2e_log "cardano-node mempool contains tx_id=$tx_id"
      return
    fi
    sleep "$TX_MEMPOOL_POLL_INTERVAL_SECONDS"
  done
  die "tx_id=$tx_id did not diffuse to cardano-node within ${timeout}s"
}

run_transaction_test() {
  local socket address utxo_file protocol_params_file tx_body tx_signed tx_cbor
  local response_file mempool_response_file input_available_slot record tx_in lovelace tx_id mempool_state submitted_at
  local -a network_args=()
  socket="$(cardano_node_socket_file)"
  utxo_file="$PRIVATE_DIR/utxo.json"
  protocol_params_file="$PRIVATE_DIR/protocol-params.json"
  tx_body="$PRIVATE_DIR/tx.body"
  tx_signed="$PRIVATE_DIR/tx.signed"
  tx_cbor="$PRIVATE_DIR/tx.cbor"
  response_file="$RESULT_DIR/submit-response.json"
  mempool_response_file="$RESULT_DIR/cardano-node-mempool.json"

  mkdir -p "$PRIVATE_DIR" "$RESULT_DIR"
  TX_PAYMENT_SKEY="$(resolve_payment_skey "$PRIVATE_DIR")"
  prepare_tx_metadata "$PRIVATE_DIR"
  address="$(payment_address "$PRIVATE_DIR/payment.vkey")"
  e2e_log "using payment address $address"

  wait_for_transaction_input "$socket" "$address" "$utxo_file"
  query_protocol_parameters "$socket" "$protocol_params_file"
  input_available_slot="$(cardano_node_tip_slot)"
  [[ "$input_available_slot" =~ ^[0-9]+$ ]] || die "could not determine cardano-node tip slot"
  record="$SELECTED_TX_RECORD"
  IFS=$'\t' read -r tx_in lovelace <<<"$record"
  e2e_log "selected input $tx_in with $lovelace lovelace at slot $input_available_slot"

  while IFS= read -r arg; do
    network_args+=("$arg")
  done < <(cardano_cli_network_args)
  build_drain_transaction "$tx_in" "$lovelace" "$address" "$tx_body" "$protocol_params_file"
  "$CARDANO_CLI" conway transaction sign \
    "${network_args[@]}" \
    --tx-body-file "$tx_body" \
    --signing-key-file "$TX_PAYMENT_SKEY" \
    --out-canonical-cbor \
    --out-file "$tx_signed"
  jq -er '.cborHex' "$tx_signed" | xxd -r -p >"$tx_cbor"
  tx_id="$("$CARDANO_CLI" conway transaction txid --tx-file "$tx_signed" --output-text)"
  [[ "$tx_id" =~ ^[0-9a-fA-F]{64}$ ]] || die "cardano-cli returned an invalid transaction id: $tx_id"
  e2e_log "built tx_id=$tx_id"

  mempool_state="$(cardano_node_mempool_tx_state "$tx_id" "$mempool_response_file")" ||
    die "cardano-node returned an invalid tx-mempool response for tx_id=$tx_id"
  [[ "$mempool_state" == absent ]] ||
    die "new tx_id=$tx_id unexpectedly existed in cardano-node's mempool before submission"
  e2e_log "pre-submit mempool check: tx_id=$tx_id is absent from cardano-node"
  wait_for_amaru_slot "$AMARU_LOG_FILE" "E2E" "$input_available_slot" "$TX_SYNC_TIMEOUT_SECONDS"
  submit_tx_and_expect_id "$tx_cbor" "$tx_id" "$response_file"
  wait_for_cardano_mempool "$tx_id" "$mempool_response_file"

  submitted_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  jq -n \
    --arg network "$NETWORK" \
    --arg tx_id "$tx_id" \
    --arg tx_in "$tx_in" \
    --arg address "$address" \
    --arg submitted_at "$submitted_at" \
    --argjson input_available_slot "$input_available_slot" \
    '{
      outcome: "passed",
      network: $network,
      tx_id: $tx_id,
      tx_in: $tx_in,
      address: $address,
      input_available_slot: $input_available_slot,
      submit_http_status: 202,
      cardano_node_mempool_before_submission: "absent",
      cardano_node_mempool: "present",
      submitted_at: $submitted_at
    }' >"$RESULT_DIR/result.json"
  e2e_success "PASS: Submit API accepted tx_id=$tx_id and the connected cardano-node received it"
  e2e_log "result: $RESULT_DIR/result.json"
}

write_failure_result() {
  local status="$1" failed_at failure_message result_tmp
  failed_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  failure_message="${E2E_FAILURE_MESSAGE:-command exited without an explicit error}"
  mkdir -p "$RESULT_DIR" || return 1
  result_tmp="$(mktemp "$RESULT_DIR/result.XXXXXX")" || return 1
  if ! jq -n \
    --arg network "$NETWORK" \
    --arg run_id "$RUN_ID" \
    --arg error "$failure_message" \
    --arg failed_at "$failed_at" \
    --argjson exit_code "$status" \
    '{
      outcome: "failed",
      network: $network,
      run_id: $run_id,
      exit_code: $exit_code,
      error: $error,
      failed_at: $failed_at
    }' >"$result_tmp"; then
    rm -f "$result_tmp"
    return 1
  fi
  if ! mv "$result_tmp" "$RESULT_DIR/result.json"; then
    rm -f "$result_tmp"
    return 1
  fi
}

cleanup() {
  local status=$?
  trap - EXIT INT TERM
  if [[ -n "$AMARU_PID" ]] && kill -0 "$AMARU_PID" 2>/dev/null; then
    kill "$AMARU_PID" 2>/dev/null || true
    wait "$AMARU_PID" 2>/dev/null || true
  fi
  if [[ -n "$CARDANO_NODE_PID" ]] && kill -0 "$CARDANO_NODE_PID" 2>/dev/null; then
    kill "$CARDANO_NODE_PID" 2>/dev/null || true
    wait "$CARDANO_NODE_PID" 2>/dev/null || true
  fi
  [[ "$TX_PAYMENT_SKEY_INSTALLED" == false ]] || rm -f "$TX_PAYMENT_SKEY"
  rm -rf "$PRIVATE_DIR"
  if ((status != 0)); then
    if ! write_failure_result "$status"; then
      e2e_warning "could not write failure result to $RESULT_DIR/result.json"
    fi
    printf '%s[e2e] failed; logs are in %s and partial results are in %s%s\n' \
      "$E2E_COLOR_ERROR" "$LOGDIR" "$RESULT_DIR" "$E2E_COLOR_RESET" >&2
  fi
  exit "$status"
}

runner_self_test() {
  local work tx_id state selected managed_before binary
  work="$(mktemp -d)"
  tx_id=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
  printf '"%s"\n' "$tx_id" >"$work/submit.json"
  submit_tx_response_matches_id "$tx_id" "$work/submit.json" ||
    die "Submit API response parser rejected the expected transaction id"
  printf '{"exists":true,"txId":"%s","slot":1}\n' "$tx_id" >"$work/mempool.json"
  state="$(parse_cardano_node_mempool_tx_state "$tx_id" "$work/mempool.json")"
  [[ "$state" == present ]] || die "expected present, got $state"
  printf '{"exists":false,"txId":"%s","slot":1}\n' "$tx_id" >"$work/mempool.json"
  state="$(parse_cardano_node_mempool_tx_state "$tx_id" "$work/mempool.json")"
  [[ "$state" == absent ]] || die "expected absent, got $state"
  printf '{"exists":true,"txId":"%s","slot":1}\n' "${tx_id%?}0" >"$work/mempool.json"
  if parse_cardano_node_mempool_tx_state "$tx_id" "$work/mempool.json" >/dev/null 2>&1; then
    die "cardano-node mempool parser accepted the wrong transaction id"
  fi
  if submit_tx_response_matches_id "${tx_id%?}0" "$work/submit.json"; then
    die "Submit API response parser accepted the wrong transaction id"
  fi
  submit_tx_response_is_duplicate 'Transaction is a duplicate.' ||
    die "Submit API duplicate response matcher rejected the expected response"
  if submit_tx_response_is_duplicate 'Transaction input is missing.'; then
    die "Submit API duplicate response matcher accepted a different rejection"
  fi
  RESULT_DIR="$work/results" E2E_FAILURE_MESSAGE="expected failure" write_failure_result 17
  jq -e '
    .outcome == "failed"
      and .exit_code == 17
      and .error == "expected failure"
  ' "$work/results/result.json" >/dev/null || die "failure result is not machine-readable"
  printf '%s\n' \
    '{"small#0":{"value":{"lovelace":2000000}},"asset#0":{"value":{"lovelace":3000000,"policy":{"token":1}}},"large#0":{"value":{"lovelace":4000000}}}' \
    >"$work/utxo.json"
  selected="$(select_transaction_input "$work/utxo.json")"
  [[ "$selected" == $'small#0\t2000000' ]] || die "transaction input selector returned: $selected"
  printf '{}\n' >"$work/utxo.json"
  if select_transaction_input "$work/utxo.json" >/dev/null; then
    die "transaction input selector accepted an empty UTxO set"
  fi
  printf '%s\n' 'tip.adopt slot=42' >"$work/amaru.log"
  wait_for_amaru_slot "$work/amaru.log" "self-test" 42 1 >/dev/null
  managed_before="$AMARU_MANAGED"
  AMARU_MANAGED=false
  start_amaru >/dev/null
  [[ -z "$AMARU_PID" ]] || die "externally managed Amaru mode started a process"
  AMARU_MANAGED="$managed_before"
  binary="$(CARGO_TARGET_DIR="$work/target" CARGO_BUILD_TARGET=x86_64-unknown-linux-gnu BUILD_PROFILE=test amaru_binary)"
  [[ "$binary" == "$work/target/x86_64-unknown-linux-gnu/debug/amaru" ]] ||
    die "Amaru binary path ignored CARGO_BUILD_TARGET: $binary"
  rm -rf "$work"
  self_test_success "response parsers, duplicate handling, input selection, slot wait, binary path, and external Amaru mode passed"
}

run_e2e() {
  trap cleanup EXIT
  trap 'exit 130' INT TERM
  install_base64_payment_key
  runner_setup
  prepare_cardano_database
  start_cardano_node
  wait_for_cardano_node
  start_amaru
  wait_for_amaru_submit_api
  run_transaction_test
}

case "${1:-}" in
  wallet) runner_wallet ;;
  snapshot) runner_snapshot ;;
  prepare) runner_prepare ;;
  setup) runner_setup ;;
  run) run_e2e ;;
  self-test) runner_self_test ;;
  -h | --help | help) usage ;;
  *) usage >&2; exit 2 ;;
esac
