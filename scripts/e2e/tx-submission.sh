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
e2e_log() { printf '%s[e2e]%s %s\n' "$E2E_COLOR_INFO" "$E2E_COLOR_RESET" "$*"; }
e2e_success() { printf '%s[e2e] %s%s\n' "$E2E_COLOR_SUCCESS" "$*" "$E2E_COLOR_RESET"; }
e2e_warning() { printf '%s[e2e] %s%s\n' "$E2E_COLOR_WARNING" "$*" "$E2E_COLOR_RESET"; }
self_test_success() { printf '%s[self-test] %s%s\n' "$E2E_COLOR_SUCCESS" "$*" "$E2E_COLOR_RESET"; }

AMARU_CHAIN_DIR="${AMARU_CHAIN_DIR:-$RUNDIR/amaru/chain.$NETWORK.db}"
AMARU_LEDGER_DIR="${AMARU_LEDGER_DIR:-$RUNDIR/amaru/ledger.$NETWORK.db}"
AMARU_LOG_FILE="${AMARU_LOG_FILE:-$LOGDIR/amaru.log}"
AMARU_LISTEN_ADDRESS="${AMARU_LISTEN_ADDRESS:-127.0.0.1:4001}"
AMARU_SUBMIT_API_ADDRESS="${AMARU_SUBMIT_API_ADDRESS:-127.0.0.1:8090}"
AMARU_PEER_ADDRESS="${AMARU_PEER_ADDRESS:-}"
AMARU_UPSTREAM_PEERS="${AMARU_UPSTREAM_PEERS:-1}"
AMARU_MANAGED="${E2E_TX_MANAGE_AMARU:-true}"

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
TX_QUERY_SOURCE=koios
TX_METADATA_MESSAGE="${TX_METADATA_MESSAGE:-amaru e2e $RUN_ID}"
TX_SYNC_TIMEOUT_SECONDS="${TX_SYNC_TIMEOUT_SECONDS:-3600}"
TX_SYNC_POLL_INTERVAL_SECONDS="${TX_SYNC_POLL_INTERVAL_SECONDS:-15}"
TX_SUBMIT_RETRY_LIMIT="${TX_SUBMIT_RETRY_LIMIT:-20}"
TX_SUBMIT_RETRY_DELAY="${TX_SUBMIT_RETRY_DELAY:-5}"
TX_INPUT_TIMEOUT_SECONDS="${TX_INPUT_TIMEOUT_SECONDS:-900}"
TX_INPUT_POLL_INTERVAL_SECONDS="${TX_INPUT_POLL_INTERVAL_SECONDS:-2}"
TX_CONFIRM_TIMEOUT_SECONDS="${TX_CONFIRM_TIMEOUT_SECONDS:-600}"
TX_CONFIRM_POLL_INTERVAL_SECONDS="${TX_CONFIRM_POLL_INTERVAL_SECONDS:-10}"

. "$COMMON_DIR/common.sh"
. "$COMMON_DIR/cardano-cli.sh"
. "$COMMON_DIR/amaru.sh"
. "$COMMON_DIR/tx.sh"

E2E_FAILURE_MESSAGE=""

die() {
  E2E_FAILURE_MESSAGE="$*"
  printf '%serror:%s %s\n' "$E2E_COLOR_ERROR" "$E2E_COLOR_RESET" "$*" >&2
  exit 1
}

AMARU_PID=""
TX_PAYMENT_SKEY_INSTALLED=false

usage() {
  cat <<'EOF'
Usage: scripts/e2e/tx-submission.sh <wallet|setup|run|self-test>

  wallet     Create the dedicated development payment key and print its faucet address.
  setup      Download cardano-cli, create the wallet, build Amaru, and bootstrap its databases.
  run        Start Amaru, submit one transaction, and verify confirmation through Koios.
  self-test  Test the strict response parsers without starting Amaru.

AMARU_PEER_ADDRESS defaults to a public peer for preprod and preview. Set it
explicitly for mainnet. Set E2E_TX_MANAGE_AMARU=false when Amaru is already
running with its Submit API enabled.
EOF
}

cardano_cli_network_args() {
  case "$NETWORK" in
    preprod) printf '%s\n' --testnet-magic 1 ;;
    preview) printf '%s\n' --testnet-magic 2 ;;
    mainnet) printf '%s\n' --mainnet ;;
    *) die "unsupported network for public transaction submission: $NETWORK" ;;
  esac
}

require_cardano_cli() {
  [[ -x "$CARDANO_CLI" ]] || die "CARDANO_CLI is not executable: $CARDANO_CLI"
}

resolve_public_peer() {
  [[ -n "$AMARU_PEER_ADDRESS" ]] && return
  case "$NETWORK" in
    preprod) AMARU_PEER_ADDRESS=preprod-node.play.dev.cardano.org:3001 ;;
    preview) AMARU_PEER_ADDRESS=preview-node.play.dev.cardano.org:3001 ;;
    mainnet) die "set AMARU_PEER_ADDRESS to a public mainnet peer" ;;
    *) die "unsupported network for public transaction submission: $NETWORK" ;;
  esac
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
  cardano_cli_network_args >/dev/null
  ensure_cardano_cli
  require_cardano_cli
  ensure_payment_wallet
  validate_configured_tx_inputs
}

runner_setup() {
  runner_wallet
  mkdir -p "$RESULTS_DIR"
  resolve_public_peer
  ensure_amaru_binary
  ensure_amaru_databases
  setup_log "transaction submission E2E prerequisites are ready"
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
  local address="$1" utxo_file="$2"
  local timeout="$TX_INPUT_TIMEOUT_SECONDS" interval="$TX_INPUT_POLL_INTERVAL_SECONDS" elapsed record

  for ((elapsed = 0; elapsed < timeout; elapsed += interval)); do
    if query_address_utxo "" "$address" "$utxo_file" && record="$(select_transaction_input "$utxo_file")"; then
      SELECTED_TX_RECORD="$record"
      return
    fi
    if ((elapsed % 30 == 0)); then
      e2e_warning "waiting for a spendable UTxO at $address (${elapsed}s/${timeout}s)"
    fi
    sleep "$interval"
  done
  die "no pure-ADA UTxO covering the minimum output and fee became visible at $address within ${timeout}s"
}

parse_koios_transaction_confirmations() {
  local tx_id="$1" response_file="$2"
  jq -r --arg tx_id "$tx_id" '
    if type != "array" then error("expected a Koios transaction status array")
    elif length == 0 then 0
    else
      [.[] | select((.tx_hash | ascii_downcase) == ($tx_id | ascii_downcase))
        | if has("num_confirmations") then .num_confirmations
          else error("Koios returned an invalid transaction status") end]
      | if length == 1 and (.[0] == null or (.[0] | type == "number" and . >= 0)) then .[0] // 0
        else error("Koios returned an invalid transaction status") end
    end
  ' "$response_file"
}

koios_transaction_confirmations() {
  local tx_id="$1" response_file="$2"
  curl --max-time "${KOIOS_TIMEOUT_SECONDS:-30}" -fsSL -X POST "$KOIOS_API_URL/tx_status" \
    -H 'accept: application/json' \
    -H 'content-type: application/json' \
    -d "$(jq -cn --arg tx_id "$tx_id" '{_tx_hashes: [$tx_id]}')" \
    -o "$response_file" || return 1
  parse_koios_transaction_confirmations "$tx_id" "$response_file"
}

wait_for_koios_confirmation() {
  local tx_id="$1" response_file="$2" elapsed confirmations
  for ((elapsed = 0; elapsed < TX_CONFIRM_TIMEOUT_SECONDS; elapsed += TX_CONFIRM_POLL_INTERVAL_SECONDS)); do
    if confirmations="$(koios_transaction_confirmations "$tx_id" "$response_file")" && ((confirmations > 0)); then
      printf '%s\n' "$confirmations"
      return 0
    fi
    sleep "$TX_CONFIRM_POLL_INTERVAL_SECONDS"
  done
  return 1
}

run_transaction_test() {
  local address utxo_file protocol_params_file tx_body tx_signed tx_cbor
  local response_file upstream_response_file input_available_slot record tx_in lovelace tx_id prior_confirmations submitted_at
  local confirmations
  local -a network_args=()
  utxo_file="$PRIVATE_DIR/utxo.json"
  protocol_params_file="$PRIVATE_DIR/protocol-params.json"
  tx_body="$PRIVATE_DIR/tx.body"
  tx_signed="$PRIVATE_DIR/tx.signed"
  tx_cbor="$PRIVATE_DIR/tx.cbor"
  response_file="$RESULT_DIR/submit-response.json"
  upstream_response_file="$RESULT_DIR/upstream-response.json"

  mkdir -p "$PRIVATE_DIR" "$RESULT_DIR"
  TX_PAYMENT_SKEY="$(resolve_payment_skey "$PRIVATE_DIR")"
  prepare_tx_metadata "$PRIVATE_DIR"
  address="$(payment_address "$PRIVATE_DIR/payment.vkey")"
  e2e_log "using payment address $address"

  wait_for_transaction_input "$address" "$utxo_file"
  query_upstream_protocol_parameters "" "$protocol_params_file"
  input_available_slot="$(koios_tip_slot)"
  [[ "$input_available_slot" =~ ^[0-9]+$ ]] || die "could not determine upstream tip slot"
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

  prior_confirmations=""
  for _ in {1..5}; do
    if prior_confirmations="$(koios_transaction_confirmations "$tx_id" "$upstream_response_file")"; then
      break
    fi
    sleep 5
  done
  [[ "$prior_confirmations" == 0 ]] || die "tx_id=$tx_id was already confirmed or Koios could not check it"
  e2e_log "pre-submit chain check: tx_id=$tx_id is not confirmed"
  wait_for_amaru_slot "$AMARU_LOG_FILE" "E2E" "$input_available_slot" "$TX_SYNC_TIMEOUT_SECONDS"
  submit_tx_and_expect_id "$tx_cbor" "$tx_id" "$response_file"
  confirmations="$(wait_for_koios_confirmation "$tx_id" "$upstream_response_file")" ||
    die "tx_id=$tx_id was not confirmed on chain within ${TX_CONFIRM_TIMEOUT_SECONDS}s"
  e2e_log "Koios reports $confirmations confirmation(s) for tx_id=$tx_id"

  submitted_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  jq -n \
    --arg network "$NETWORK" \
    --arg tx_id "$tx_id" \
    --arg tx_in "$tx_in" \
    --arg address "$address" \
    --arg submitted_at "$submitted_at" \
    --argjson confirmations "$confirmations" \
    --argjson input_available_slot "$input_available_slot" \
    '{
      outcome: "passed",
      network: $network,
      tx_id: $tx_id,
      tx_in: $tx_in,
      address: $address,
      input_available_slot: $input_available_slot,
      submit_http_status: 202,
      upstream_verification: {
        source: "koios",
        status: "confirmed",
        confirmations: $confirmations
      },
      submitted_at: $submitted_at
    }' >"$RESULT_DIR/result.json"
  e2e_success "PASS: Submit API accepted tx_id=$tx_id; Koios confirmed it on chain"
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
  local work tx_id selected managed_before binary confirmations
  work="$(mktemp -d)"
  tx_id=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
  printf '"%s"\n' "$tx_id" >"$work/submit.json"
  submit_tx_response_matches_id "$tx_id" "$work/submit.json" ||
    die "Submit API response parser rejected the expected transaction id"
  printf '[]\n' >"$work/koios.json"
  confirmations="$(parse_koios_transaction_confirmations "$tx_id" "$work/koios.json")"
  [[ "$confirmations" == 0 ]] || die "expected an unconfirmed Koios transaction, got $confirmations"
  printf '[{"tx_hash":"%s","num_confirmations":null}]\n' "$tx_id" >"$work/koios.json"
  confirmations="$(parse_koios_transaction_confirmations "$tx_id" "$work/koios.json")"
  [[ "$confirmations" == 0 ]] || die "expected null Koios confirmations to mean zero, got $confirmations"
  printf '[{"tx_hash":"%s","num_confirmations":1}]\n' "$tx_id" >"$work/koios.json"
  confirmations="$(parse_koios_transaction_confirmations "$tx_id" "$work/koios.json")"
  [[ "$confirmations" == 1 ]] || die "expected one Koios confirmation, got $confirmations"
  printf '[{"tx_hash":"%s"}]\n' "$tx_id" >"$work/koios.json"
  if parse_koios_transaction_confirmations "$tx_id" "$work/koios.json" >/dev/null 2>&1; then
    die "Koios transaction status parser accepted a missing confirmation count"
  fi
  printf '[{"tx_hash":"%s","num_confirmations":1}]\n' "${tx_id%?}0" >"$work/koios.json"
  if parse_koios_transaction_confirmations "$tx_id" "$work/koios.json" >/dev/null 2>&1; then
    die "Koios transaction status parser accepted the wrong transaction id"
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
  wait_for_upstream_ready
  start_amaru
  wait_for_amaru_submit_api
  run_transaction_test
}

case "${1:-}" in
  wallet) runner_wallet ;;
  setup) runner_setup ;;
  run) run_e2e ;;
  self-test) runner_self_test ;;
  -h | --help | help) usage ;;
  *) usage >&2; exit 2 ;;
esac
