#!/usr/bin/env bash

# Transaction configuration. Callers may override these with environment variables.
TX_PAYMENT_SKEY="${TX_PAYMENT_SKEY:-}"
TX_GENERATED_COUNT="${TX_GENERATED_COUNT:-1}"
TX_SYNC_TIMEOUT_SECONDS="${TX_SYNC_TIMEOUT_SECONDS:-14400}"
TX_SUBMIT_RETRY_LIMIT="${TX_SUBMIT_RETRY_LIMIT:-12}"
TX_SUBMIT_RETRY_DELAY="${TX_SUBMIT_RETRY_DELAY:-30}"
TX_OUTPUT_LOVELACE="${TX_OUTPUT_LOVELACE:-1000000}"
TX_FEE_BUFFER_LOVELACE="${TX_FEE_BUFFER_LOVELACE:-300000}"
TX_REFUEL_UTXO_COUNT="${TX_REFUEL_UTXO_COUNT:-10}"
TX_REFUEL_OUTPUT_LOVELACE="${TX_REFUEL_OUTPUT_LOVELACE:-2000000}"
TX_REFUEL_MAX_INPUTS="${TX_REFUEL_MAX_INPUTS:-80}"
TX_REFUEL_CONFIRM_TIMEOUT_SECONDS="${TX_REFUEL_CONFIRM_TIMEOUT_SECONDS:-300}"
TX_REFUEL_SELECTION="${TX_REFUEL_SELECTION:-largest}"
TX_REFUEL_MIN_CHANGE_LOVELACE="${TX_REFUEL_MIN_CHANGE_LOVELACE:-1000000}"
TX_METADATA_MESSAGE="${TX_METADATA_MESSAGE-made by amaru with 💜}"

# CIP-20 defines label 674 with a `msg` array for human-readable transaction messages, which is what
# explorers render as a transaction comment.
TX_METADATA_LABEL=674

# The ledger rejects any metadata text string longer than this, counted in bytes: an emoji costs
# several, so the limit bites well before the message looks long.
TX_METADATA_MAX_BYTES=64
TX_REFUEL_FORCE="${TX_REFUEL_FORCE:-false}"
TX_REFUEL_CARDANO_SYNC_PROGRESS="${TX_REFUEL_CARDANO_SYNC_PROGRESS:-99.9}"
TX_REFUEL_CARDANO_SYNC_TIMEOUT_SECONDS="${TX_REFUEL_CARDANO_SYNC_TIMEOUT_SECONDS:-14400}"
TX_QUERY_SOURCE="${TX_QUERY_SOURCE:-local}"

default_koios_api_url() {
  case "${NETWORK:-}" in
  preprod) echo "https://preprod.koios.rest/api/v1" ;;
  preview) echo "https://preview.koios.rest/api/v1" ;;
  *) echo "https://api.koios.rest/api/v1" ;;
  esac
}

KOIOS_API_URL="${KOIOS_API_URL:-$(default_koios_api_url)}"

# Writes the CIP-20 message metadata attached to generated transactions and points TX_METADATA_FILE
# at it. An empty TX_METADATA_MESSAGE attaches no metadata at all.
prepare_tx_metadata() {
  local dir="$1"

  TX_METADATA_FILE=""
  [[ -n "$TX_METADATA_MESSAGE" ]] || return 0
  TX_METADATA_FILE="$dir/metadata.json"
  jq -n --argjson label "$TX_METADATA_LABEL" --arg message "$TX_METADATA_MESSAGE" \
    '{($label | tostring): {msg: [$message]}}' >"$TX_METADATA_FILE"
}

# Expands to the metadata option for a transaction build, or to nothing when there is no message.
# Every build of a given transaction must include it: metadata changes the serialized size, so a
# draft built without it would under-estimate the fee and the ledger would reject the result.
tx_metadata_args() {
  [[ -n "${TX_METADATA_FILE:-}" ]] || return 0
  printf '%s\n' --metadata-json-file "$TX_METADATA_FILE"
}

# Returns whether runtime transaction generation is configured.
tx_generation_enabled() {
  [[ -n "$TX_PAYMENT_SKEY" ]]
}

# Validates optional runtime transaction generation settings before startup.
validate_configured_tx_inputs() {
  if tx_generation_enabled; then
    have jq || die "jq is required to generate transactions"
    have curl || die "curl not found"
    have xxd || die "xxd is required to encode canonical transaction CBOR"
    payment_skey_is_inline || [[ -f "$TX_PAYMENT_SKEY" ]] ||
      die "TX_PAYMENT_SKEY is neither a readable file nor key material: $TX_PAYMENT_SKEY"
    # cardano-cli converts a root key as readily as a payment key, to an address that will never hold
    # the demo funds, so the rejection has to happen here.
    [[ "$(payment_skey_format)" != "cardano-address-root" ]] || die "TX_PAYMENT_SKEY is a cardano-address root key: $TX_PAYMENT_SKEY; derive the payment key (addr_xsk) from it first"
    local message_bytes
    message_bytes=$(printf '%s' "$TX_METADATA_MESSAGE" | wc -c | tr -d ' ')
    ((message_bytes <= TX_METADATA_MAX_BYTES)) ||
      die "TX_METADATA_MESSAGE is $message_bytes bytes, over the $TX_METADATA_MAX_BYTES the ledger allows for one metadata string: '$TX_METADATA_MESSAGE'"
  fi
}

require_configured_tx() {
  validate_configured_tx_inputs
  if tx_generation_enabled; then
    require_cardano_cli
  fi
}

tx_query_uses_koios() {
  [[ "$TX_QUERY_SOURCE" == "koios" ]]
}

submit_tx_response_is_duplicate() {
  grep -qi 'transaction is a duplicate' <<<"$1"
}

# Submits a transaction to the downstream submit API with retryable rejection handling.
submit_tx_with_retry() {
  local tx_file="$1" response_file="$2" attempt=1
  local response status body retries="$TX_SUBMIT_RETRY_LIMIT" delay="$TX_SUBMIT_RETRY_DELAY"
  while ((attempt <= retries)); do
    echo "[submit-tx] attempt $attempt/$retries: submitting $tx_file to the Amaru submit API at $TX_SUBMIT_API_ADDRESS"
    if curl -sS -o "$response_file" -w '%{http_code}' \
      -X POST -H 'Content-Type: application/cbor' \
      --data-binary "@$tx_file" \
      "http://$TX_SUBMIT_API_ADDRESS/api/submit/tx" >"$response_file.status"; then
      status="$(cat "$response_file.status")"
      body="$(cat "$response_file")"
      echo "[submit-tx] response: HTTP $status"
      echo "$body"
      if [[ "$status" == 2* ]]; then
        return 0
      fi
      if submit_tx_response_is_duplicate "$body"; then
        echo "[submit-tx] downstream already knows this transaction; keeping claim"
        return 0
      fi
      if grep -qi 'missing transaction inputs' <<<"$body"; then
        echo "[submit-tx] downstream ledger does not yet contain the input UTxO; waiting ${delay}s before retry"
      else
        echo "[submit-tx] non-retryable rejection; aborting retries for this tx"
        return 1
      fi
    else
      echo "[submit-tx] curl failed; will retry in ${delay}s"
    fi
    sleep "$delay"
    attempt=$((attempt + 1))
  done
  echo "[submit-tx] giving up after $retries attempts"
  return 1
}

# Validates the successful Submit API response for a known transaction. The endpoint returns the
# accepted transaction id as a JSON string; accepting a generic 2xx here would not prove that the
# request body was decoded as the transaction the caller built.
submit_tx_response_matches_id() {
  local expected_tx_id="$1" response_file="$2"

  jq -e --arg expected_tx_id "$expected_tx_id" '
    type == "string" and (ascii_downcase == ($expected_tx_id | ascii_downcase))
  ' "$response_file" >/dev/null
}

# Submits a transaction and verifies the complete public contract used by the end-to-end test:
# HTTP 202 and a response body containing exactly the expected transaction id. A duplicate response
# also completes the submission because an earlier attempt may have timed out after acceptance. A
# missing-input rejection is retryable because Amaru may still be catching up to the cardano-node
# used to select the input.
submit_tx_and_expect_id() {
  local tx_file="$1" expected_tx_id="$2" response_file="$3" attempt=1
  local status body retries="$TX_SUBMIT_RETRY_LIMIT" delay="$TX_SUBMIT_RETRY_DELAY"

  while ((attempt <= retries)); do
    echo "[submit-tx] attempt $attempt/$retries: submitting tx_id=$expected_tx_id to the Amaru Submit API at $TX_SUBMIT_API_ADDRESS"
    if curl --max-time "${TX_SUBMIT_HTTP_TIMEOUT_SECONDS:-30}" -sS -o "$response_file" -w '%{http_code}' \
      -X POST -H 'Content-Type: application/cbor' \
      --data-binary "@$tx_file" \
      "http://$TX_SUBMIT_API_ADDRESS/api/submit/tx" >"$response_file.status"; then
      status="$(cat "$response_file.status")"
      body="$(cat "$response_file")"
      echo "[submit-tx] response: HTTP $status"
      echo "$body"
      if [[ "$status" == 202 ]] && submit_tx_response_matches_id "$expected_tx_id" "$response_file"; then
        return 0
      fi
      if [[ "$status" == 202 ]]; then
        echo "[submit-tx] HTTP 202 response did not contain the expected transaction id"
        return 1
      fi
      if submit_tx_response_is_duplicate "$body"; then
        echo "[submit-tx] downstream already knows this transaction; submission is complete"
        return 0
      fi
      if grep -qi 'missing transaction inputs' <<<"$body"; then
        echo "[submit-tx] Amaru has not indexed the input UTxO yet; waiting ${delay}s before retry"
      else
        echo "[submit-tx] expected HTTP 202 for tx_id=$expected_tx_id, got HTTP $status; aborting retries"
        return 1
      fi
    else
      echo "[submit-tx] Submit API request failed; waiting ${delay}s before retry"
    fi
    sleep "$delay"
    attempt=$((attempt + 1))
  done
  echo "[submit-tx] giving up after $retries attempts"
  return 1
}

# Whether TX_PAYMENT_SKEY holds the key itself rather than a path to a file holding it, which lets a
# container run without mounting anything.
#
# Key material is recognised by its own shape, deliberately, rather than by the path lookup failing:
# treating an unreadable path as a key would turn a typo into a confusing complaint about the key
# instead of a plain "this file does not exist".
payment_skey_is_inline() {
  case "$TX_PAYMENT_SKEY" in
  addr_xsk1* | root_xsk1* | '{'*) return 0 ;;
  *) return 1 ;;
  esac
}

# The leading bytes of the key, from wherever it lives.
payment_skey_prefix() {
  if payment_skey_is_inline; then
    printf '%.9s' "$TX_PAYMENT_SKEY"
  else
    head -c 9 "$TX_PAYMENT_SKEY" 2>/dev/null
  fi
}

# The format of the configured signing key. cardano-address writes bech32 text whose human-readable
# prefix names the derivation level, while cardano-cli writes JSON.
payment_skey_format() {
  case "$(payment_skey_prefix)" in
  addr_xsk1) echo "cardano-address-payment" ;;
  root_xsk1) echo "cardano-address-root" ;;
  *) echo "cardano-cli" ;;
  esac
}

# Converts a cardano-address payment key into the cardano-cli format the rest of the script uses, and
# returns the path to use for signing. Other formats are passed through untouched.
#
# The converted key lands in the caller's per-process directory, which is wiped on every run, so no
# additional copy of the key outlives the process that needs it.
resolve_payment_skey() {
  local tx_dir="$1" source="$TX_PAYMENT_SKEY" converted

  # Inline key material has to reach cardano-cli as a file either way, so it is written out first
  # and the rest of the resolution then treats both forms identically.
  if payment_skey_is_inline; then
    source="$tx_dir/payment-inline.skey"
    (umask 077 && printf '%s\n' "$TX_PAYMENT_SKEY" >"$source")
  fi

  if [[ "$(payment_skey_format)" != "cardano-address-payment" ]]; then
    echo "$source"
    return 0
  fi

  converted="$tx_dir/payment.skey"
  "$CARDANO_CLI" key convert-cardano-address-key \
    --shelley-payment-key \
    --signing-key-file "$source" \
    --out-file "$converted"
  chmod 600 "$converted"
  echo "$converted"
}

# Derives the payment address used for generated transactions. The address is always derived from the
# signing key, so the demo can only ever build transactions it can also sign.
payment_address() {
  local vkey="$1"

  "$CARDANO_CLI" conway key verification-key --signing-key-file "$TX_PAYMENT_SKEY" --verification-key-file "$vkey" >/dev/null
  "$CARDANO_CLI" conway address build --payment-verification-key-file "$vkey" $(cardano_cli_network_args)
}

# Queries the UTxOs at an address from whichever source the upstream mode provides, retrying the
# Koios call because a public endpoint under load answers with a non-JSON error page.
query_address_utxo() {
  local socket="$1" address="$2" utxo_file="$3" attempt

  if ! tx_query_uses_koios; then
    query_payment_utxo "$socket" "$address" "$utxo_file"
    return
  fi

  for attempt in {1..60}; do
    if query_koios_payment_utxo "$address" "$utxo_file"; then
      return 0
    fi
    echo "[tx] Koios UTxO query failed (attempt $attempt); retrying..."
    sleep 5
  done
  return 1
}

# Queries the protocol parameters from whichever source the upstream mode provides.
query_upstream_protocol_parameters() {
  local socket="$1" protocol_params_file="$2"

  if tx_query_uses_koios; then
    query_koios_protocol_parameters "$protocol_params_file"
  else
    query_protocol_parameters "$socket" "$protocol_params_file"
  fi
}

# Submits a signed transaction to the upstream network: through Koios when no local node runs,
# otherwise through the node socket. Koios takes the raw CBOR bytes, so the caller passes the
# canonical CBOR file alongside the cardano-cli JSON.
submit_tx_upstream() {
  local socket="$1" tx_signed="$2" tx_cbor="$3" error_file="$4" status

  if tx_query_uses_koios; then
    # Deliberately not `curl -f`: on a rejection the response body carries the ledger's reason, and
    # -f discards it, leaving nothing but an HTTP status to debug from.
    status="$(
      curl --max-time "${KOIOS_TIMEOUT_SECONDS:-30}" -sS -X POST "$KOIOS_API_URL/submittx" \
        -H 'content-type: application/cbor' \
        --data-binary "@$tx_cbor" \
        -o "$error_file" -w '%{http_code}' 2>>"$error_file"
    )"
    [[ "$status" == 2* ]] || {
      printf '\n[submit] %s answered HTTP %s for a %s byte transaction\n' \
        "$KOIOS_API_URL/submittx" "$status" "$(wc -c <"$tx_cbor" | tr -d ' ')" >>"$error_file"
      return 1
    }
  else
    "$CARDANO_CLI" conway transaction submit \
      $(cardano_cli_network_args) \
      --socket-path "$socket" \
      --tx-file "$tx_signed" 2>"$error_file"
  fi
}

# Waits until the upstream is current enough to answer wallet queries: a sync-progress threshold
# for a local node, or simply a reachable tip for Koios, which only ever serves the real tip.
wait_for_upstream_ready() {
  if tx_query_uses_koios; then
    [[ -n "$(koios_tip_slot || true)" ]] || die "could not reach Koios at $KOIOS_API_URL"
    return 0
  fi
  wait_for_cardano_socket
  wait_for_cardano_query
  wait_for_cardano_sync_progress "$TX_REFUEL_CARDANO_SYNC_PROGRESS" "$TX_REFUEL_CARDANO_SYNC_TIMEOUT_SECONDS"
}

query_protocol_parameters() {
  local socket="$1" protocol_params_file="$2"
  "$CARDANO_CLI" conway query protocol-parameters \
    $(cardano_cli_network_args) \
    --socket-path "$socket" \
    --out-file "$protocol_params_file"
}

query_payment_utxo() {
  local socket="$1" address="$2" utxo_file="$3"
  "$CARDANO_CLI" conway query utxo \
    $(cardano_cli_network_args) \
    --socket-path "$socket" \
    --address "$address" \
    --output-json \
    --out-file "$utxo_file"
}

koios_tip_slot() {
  curl --max-time "${KOIOS_TIMEOUT_SECONDS:-30}" -fsSL \
    -H 'accept: application/json' \
    "$KOIOS_API_URL/tip" | jq -r '.[0].abs_slot // empty'
}

query_koios_protocol_parameters() {
  local protocol_params_file="$1"
  curl --max-time "${KOIOS_TIMEOUT_SECONDS:-30}" -fsSL \
    -H 'accept: application/json' \
    "$KOIOS_API_URL/cli_protocol_params" >"$protocol_params_file"
}

query_koios_payment_utxo() {
  local address="$1" utxo_file="$2"
  curl --max-time "${KOIOS_TIMEOUT_SECONDS:-30}" -fsSL -X POST "$KOIOS_API_URL/address_info" \
    -H 'accept: application/json' \
    -H 'Content-Type: application/json' \
    -d "$(jq -cn --arg address "$address" '{_addresses: [$address]}')" |
    jq --arg address "$address" '
        (.[0].utxo_set // [])
        | map(select((.asset_list // []) | length == 0))
        | map({
            key: (.tx_hash + "#" + ((.tx_index | tostring))),
            value: {
              address: $address,
              value: { lovelace: (.value | tonumber) }
            }
          })
        | from_entries
      ' >"$utxo_file"
}

# Builds the wallet preparation transaction without a node to balance it: N exact outputs of
# TX_REFUEL_OUTPUT_LOVELACE plus a change output, with the fee calculated from a same-size draft.
#
# The refuel outputs must be exact, because count_clean_refuel_outputs recognises clean outputs by
# their precise lovelace value, so any remainder goes to change. A change output too small to be
# worth creating is added to the fee instead, which the ledger accepts.
build_refuel_transaction() {
  local total_lovelace="$1" address="$2" tx_body="$3" protocol_params_file="$4"
  shift 4
  local -a tx_in_args=("$@")
  local draft_body="$tx_body.draft" fee change outputs_lovelace i arg
  local -a out_args=()

  # This function owns the outputs, because the fee and change arithmetic depends on knowing exactly
  # how many there are. Outputs arriving from the caller as well would balance against a total that
  # does not include them, and the ledger rejects that as ValueNotConservedUTxO.
  for arg in "${tx_in_args[@]}"; do
    [[ "$arg" != --tx-out ]] || die "build_refuel_transaction takes inputs only, but was given --tx-out"
  done

  for ((i = 0; i < TX_REFUEL_UTXO_COUNT; i++)); do
    out_args+=(--tx-out "$address+$TX_REFUEL_OUTPUT_LOVELACE")
  done
  outputs_lovelace=$((TX_REFUEL_UTXO_COUNT * TX_REFUEL_OUTPUT_LOVELACE))

  if ((total_lovelace <= outputs_lovelace)); then
    echo "[prepare-wallet] $total_lovelace lovelace cannot cover ${TX_REFUEL_UTXO_COUNT}x${TX_REFUEL_OUTPUT_LOVELACE} lovelace of outputs"
    return 1
  fi

  # The draft only has to match the final transaction's serialized size, so its change output
  # carries a placeholder of the same CBOR width rather than the real amount, which is not known
  # until the fee is calculated. Dropping the change output later only makes the transaction
  # smaller, and overpaying the fee is always valid.
  "$CARDANO_CLI" conway transaction build-raw \
    "${tx_in_args[@]}" "${out_args[@]}" \
    --tx-out "$address+2000000" \
    --fee 300000 \
    $(tx_metadata_args) \
    --out-file "$draft_body"

  fee="$(
    "$CARDANO_CLI" conway transaction calculate-min-fee \
      --tx-body-file "$draft_body" \
      --protocol-params-file "$protocol_params_file" \
      --witness-count 1 \
      $(cardano_cli_network_args) \
      --output-text |
      awk '{print $1}'
  )"
  [[ "$fee" =~ ^[0-9]+$ ]] || return 1
  fee=$((fee + ${TX_FEE_MARGIN_LOVELACE:-2000}))

  change=$((total_lovelace - outputs_lovelace - fee))
  ((change >= 0)) || {
    echo "[prepare-wallet] $total_lovelace lovelace cannot cover ${TX_REFUEL_UTXO_COUNT}x${TX_REFUEL_OUTPUT_LOVELACE} plus a fee of $fee"
    return 1
  }

  if ((change < TX_REFUEL_MIN_CHANGE_LOVELACE)); then
    echo "[prepare-wallet] change of $change lovelace is below TX_REFUEL_MIN_CHANGE_LOVELACE=$TX_REFUEL_MIN_CHANGE_LOVELACE; adding it to the fee instead of creating a dust output"
    fee=$((fee + change))
    change=0
    "$CARDANO_CLI" conway transaction build-raw \
      "${tx_in_args[@]}" "${out_args[@]}" \
      --fee "$fee" \
      $(tx_metadata_args) \
      --out-file "$tx_body"
  else
    "$CARDANO_CLI" conway transaction build-raw \
      "${tx_in_args[@]}" "${out_args[@]}" \
      --tx-out "$address+$change" \
      --fee "$fee" \
      $(tx_metadata_args) \
      --out-file "$tx_body"
  fi
  echo "[prepare-wallet] built preparation transaction with fee $fee and change $change lovelace"
}

build_drain_transaction() {
  local tx_in="$1" lovelace="$2" address="$3" tx_body="$4" protocol_params_file="$5"
  local draft_body="$tx_body.draft" fee output_lovelace

  # The draft must have the same serialized size as the final transaction, or the calculated
  # minimum fee comes up short: a fee of 0 encodes as a single CBOR byte while the real fee
  # takes five, and the ledger prices the four extra bytes. Any realistic placeholder in the
  # 65536..4294967295 range has the same width as the final fee.
  "$CARDANO_CLI" conway transaction build-raw \
    --tx-in "$tx_in" \
    --tx-out "$address+$lovelace" \
    --fee 300000 \
    $(tx_metadata_args) \
    --out-file "$draft_body"

  fee="$(
    "$CARDANO_CLI" conway transaction calculate-min-fee \
      --tx-body-file "$draft_body" \
      --protocol-params-file "$protocol_params_file" \
      --witness-count 1 \
      $(cardano_cli_network_args) \
      --output-text |
      awk '{print $1}'
  )"
  [[ "$fee" =~ ^[0-9]+$ ]] || return 1

  # calculate-min-fee under-estimates the signed transaction by a few bytes (the witness-set
  # encoding it assumes is smaller than what `transaction sign` produces), and the ledger
  # rejects a fee even one lovelace below the minimum. Overpaying is always valid, so add a
  # margin that covers a few dozen bytes of estimation drift.
  fee=$((fee + ${TX_FEE_MARGIN_LOVELACE:-2000}))

  output_lovelace=$((lovelace - fee))
  if ((output_lovelace < TX_OUTPUT_LOVELACE)); then
    echo "[submit-tx] cannot drain $tx_in: output $output_lovelace lovelace would be below TX_OUTPUT_LOVELACE=$TX_OUTPUT_LOVELACE"
    return 1
  fi

  "$CARDANO_CLI" conway transaction build-raw \
    --tx-in "$tx_in" \
    --tx-out "$address+$output_lovelace" \
    --fee "$fee" \
    $(tx_metadata_args) \
    --out-file "$tx_body"
  echo "[submit-tx] built drain transaction from $tx_in with fee $fee and output $output_lovelace lovelace"
}

clear_submit_claim_state() {
  rm -rf "$RUNDIR/generated/submit-tx-claims" "$RUNDIR/generated/submit-tx-claims.lock" "$RUNDIR/generated/submit-tx-txids" "$RUNDIR/generated/submit-tx-active" 2>/dev/null || true
}

count_clean_refuel_outputs() {
  local utxo_file="$1"
  jq --argjson lovelace "$TX_REFUEL_OUTPUT_LOVELACE" '
    [to_entries[] | select(.value.value.lovelace == $lovelace)] | length
  ' "$utxo_file"
}

# Counts the outputs a submit replica could actually claim, using the same threshold submit-tx
# applies when selecting an input. Wallet preparation is decided on this rather than on outputs of
# an exact size: a drained output is a few thousand lovelace short of the refuel size but remains
# perfectly spendable, and rebuilding it would spend a fee to gain nothing.
count_spendable_outputs() {
  local utxo_file="$1"
  jq --argjson lovelace "$((TX_OUTPUT_LOVELACE + TX_FEE_BUFFER_LOVELACE))" '
    [to_entries[] | select(.value.value.lovelace >= $lovelace)] | length
  ' "$utxo_file"
}

safe_claim_name() {
  local tx_in="$1"
  echo "${tx_in//[^A-Za-z0-9]/_}"
}

claim_tx_in() {
  local tx_in="$1" claim_dir="$2"
  if mkdir "$claim_dir" 2>/dev/null; then
    printf '%s\n' "$tx_in" >"$claim_dir/tx-in"
    printf '%s\n' "$$" >"$claim_dir/pid"
    return 0
  fi
  return 1
}

release_tx_claim() {
  local claim_dir="$1"
  rm -rf "$claim_dir"
}

claim_tx_id() {
  local tx_id="$1" claim_dir="$2" tx_in="$3"
  if mkdir "$claim_dir" 2>/dev/null; then
    printf '%s\n' "$tx_id" >"$claim_dir/tx-id"
    printf '%s\n' "$tx_in" >"$claim_dir/tx-in"
    printf '%s\n' "$$" >"$claim_dir/pid"
    printf '%s\n' "pending" >"$claim_dir/status"
    return 0
  fi
  return 1
}

accept_tx_id_claim() {
  local claim_dir="$1"
  printf '%s\n' "accepted" >"$claim_dir/status"
}

acquire_submit_claims_lock() {
  local lock_dir="$1" timeout=60 pid
  for ((elapsed = 0; elapsed < timeout; elapsed++)); do
    if mkdir "$lock_dir" 2>/dev/null; then
      printf '%s\n' "$$" >"$lock_dir/pid"
      return 0
    fi
    pid="$(cat "$lock_dir/pid" 2>/dev/null || true)"
    if [[ -n "$pid" ]] && ! kill -0 "$pid" 2>/dev/null; then
      rm -rf "$lock_dir"
      continue
    fi
    sleep 1
  done
  die "submit-tx claims lock was not acquired within ${timeout}s"
}

remove_inactive_submit_replicas() {
  local active_dir="$1" replica_dir pid
  [[ -d "$active_dir" ]] || return 0
  for replica_dir in "$active_dir"/*; do
    [[ -d "$replica_dir" ]] || continue
    pid="$(cat "$replica_dir/pid" 2>/dev/null || true)"
    if [[ -z "$pid" ]] || ! kill -0 "$pid" 2>/dev/null; then
      rm -rf "$replica_dir"
    fi
  done
}

remove_abandoned_tx_id_claims() {
  local tx_id_dir="$1" tx_id_claim_dir pid status
  [[ -d "$tx_id_dir" ]] || return 0
  for tx_id_claim_dir in "$tx_id_dir"/*; do
    [[ -d "$tx_id_claim_dir" ]] || continue
    status="$(cat "$tx_id_claim_dir/status" 2>/dev/null || true)"
    [[ "$status" == "accepted" ]] && continue
    pid="$(cat "$tx_id_claim_dir/pid" 2>/dev/null || true)"
    if [[ -z "$pid" ]] || ! kill -0 "$pid" 2>/dev/null; then
      rm -rf "$tx_id_claim_dir"
    fi
  done
}

initialize_submit_claims() {
  local claim_dir="$1" tx_id_dir="$2" lock_dir="$3" active_dir="$4" replica_num="$5" active_replica_dir
  active_replica_dir="$active_dir/$replica_num-$$"
  mkdir -p "$(dirname "$claim_dir")" "$tx_id_dir" "$active_dir"
  acquire_submit_claims_lock "$lock_dir"
  remove_inactive_submit_replicas "$active_dir"
  remove_abandoned_tx_id_claims "$tx_id_dir"
  if [[ -z "$(fd --hidden --no-ignore --type d --exact-depth 1 --max-results 1 . "$active_dir")" ]]; then
    echo "[submit-tx] clearing stale submit-tx claims from $claim_dir"
    rm -rf "$claim_dir"
  fi
  mkdir -p "$claim_dir" "$active_replica_dir"
  printf '%s\n' "$$" >"$active_replica_dir/pid"
  printf '%s\n' "$replica_num" >"$active_replica_dir/replica"
  rm -rf "$lock_dir"
  SUBMIT_TX_ACTIVE_REPLICA_DIR="$active_replica_dir"
}

release_submit_active_replica() {
  if [[ -n "${SUBMIT_TX_ACTIVE_REPLICA_DIR:-}" ]]; then
    rm -rf "$SUBMIT_TX_ACTIVE_REPLICA_DIR"
  fi
}

# Generates transactions from configured payment credentials and submits them.
generate_submit() {
  if [[ "$TX_GENERATED_COUNT" =~ ^0+$ ]]; then
    mkdir -p "$LOGDIR"
    exec > >(tee -a "$LOGDIR/submit-tx.log") 2>&1
    echo "[submit-tx] skipped because TX_GENERATED_COUNT=$TX_GENERATED_COUNT"
    return 0
  fi

  require_cardano_cli
  [[ -n "$CARDANO_NODE_CONFIG_DIR" ]] || die "CARDANO_NODE_CONFIG_DIR must be set"
  [[ -n "$TX_PAYMENT_SKEY" ]] || die "TX_PAYMENT_SKEY must be set to a funded payment signing key"
  payment_skey_is_inline || [[ -f "$TX_PAYMENT_SKEY" ]] ||
    die "TX_PAYMENT_SKEY is neither a readable file nor key material: $TX_PAYMENT_SKEY"
  have jq || die "jq is required to query and select UTxOs"
  have curl || die "curl not found"

  local socket replica_num
  socket="$(cardano_node_socket_file)"
  replica_num="${PC_REPLICA_NUM:-0}"
  local address tx_dir claim_dir tx_id_dir claim_lock_dir active_dir utxo_file protocol_params_file preferred_lovelace min_spendable_lovelace input_available_slot
  tx_dir="$RUNDIR/generated/submit-tx-$replica_num"
  claim_dir="$RUNDIR/generated/submit-tx-claims"
  tx_id_dir="$RUNDIR/generated/submit-tx-txids"
  claim_lock_dir="$RUNDIR/generated/submit-tx-claims.lock"
  active_dir="$RUNDIR/generated/submit-tx-active"
  utxo_file="$tx_dir/utxo.json"
  protocol_params_file="$tx_dir/protocol-params.json"
  preferred_lovelace=3000000
  min_spendable_lovelace=$((TX_OUTPUT_LOVELACE + TX_FEE_BUFFER_LOVELACE))

  mkdir -p "$tx_dir" "$LOGDIR"
  rm -f "$tx_dir"/tx-* "$tx_dir"/last-response-* "$tx_dir"/submit-result-* "$tx_dir"/protocol-params.json "$tx_dir"/utxo.json "$tx_dir"/payment.vkey "$tx_dir"/payment.skey "$tx_dir"/payment-inline.skey 2>/dev/null || true
  exec > >(tee -a "$LOGDIR/submit-tx.log") 2>&1
  validate_amaru_runtime_network "${AMARU_MIDDLE_LOG_FILE:-$LOGDIR/amaru-middle.log}" "middle"
  validate_amaru_runtime_network "${AMARU_DOWNSTREAM_LOG_FILE:-$LOGDIR/amaru-downstream.log}" "downstream"
  initialize_submit_claims "$claim_dir" "$tx_id_dir" "$claim_lock_dir" "$active_dir" "$replica_num"
  trap release_submit_active_replica EXIT

  TX_PAYMENT_SKEY="$(resolve_payment_skey "$tx_dir")"
  prepare_tx_metadata "$tx_dir"
  address="$(payment_address "$tx_dir/payment.vkey")"
  echo "[submit-tx] using payment address: $address"
  [[ -z "$TX_METADATA_MESSAGE" ]] || echo "[submit-tx] attaching metadata label $TX_METADATA_LABEL message: $TX_METADATA_MESSAGE"
  local queried=false
  if tx_query_uses_koios; then
    echo "[submit-tx] querying UTxO and protocol parameters from Koios..."
    for _ in {1..60}; do
      if query_koios_payment_utxo "$address" "$utxo_file"; then
        queried=true
        break
      fi
      sleep 1
    done
    [[ "$queried" == true ]] || die "could not query UTxO from Koios for address: $address"
    query_koios_protocol_parameters "$protocol_params_file"
    input_available_slot="$(koios_tip_slot || true)"
    [[ -n "$input_available_slot" ]] || die "could not query Koios tip after selecting UTxOs"
  else
    echo "[submit-tx] waiting for upstream cardano-node socket to answer local queries..."
    wait_for_cardano_socket
    wait_for_cardano_query

    echo "[submit-tx] querying UTxO from upstream cardano-node socket..."
    for _ in {1..60}; do
      if query_payment_utxo "$socket" "$address" "$utxo_file"; then
        queried=true
        break
      fi
      sleep 1
    done
    [[ "$queried" == true ]] || die "could not query UTxO from cardano-node socket: $socket"
    query_protocol_parameters "$socket" "$protocol_params_file"
    input_available_slot="$(cardano_node_tip_slot || true)"
    [[ -n "$input_available_slot" ]] || die "could not query cardano-node tip after selecting UTxOs"
  fi
  echo "[submit-tx] selected UTxO set is available at upstream slot $input_available_slot"

  local -a tx_records=()
  while IFS= read -r record; do
    tx_records+=("$record")
  done < <(
    jq -r '
      to_entries[]
      | [.key, (.value.value.lovelace // 0)]
      | @tsv
    ' "$utxo_file" | sort -k2,2n
  )

  [[ ${#tx_records[@]} -gt 0 ]] || die "no UTxO found for $address"

  local -a candidate_records=()
  local has_preferred=false
  local record tx_in lovelace candidate_claim_dir
  for record in "${tx_records[@]}"; do
    IFS=$'\t' read -r tx_in lovelace <<<"$record"
    if [[ "$lovelace" -ge "$preferred_lovelace" ]]; then
      candidate_records+=("$record")
      has_preferred=true
    fi
  done
  for record in "${tx_records[@]}"; do
    IFS=$'\t' read -r tx_in lovelace <<<"$record"
    if [[ "$lovelace" -lt "$preferred_lovelace" ]]; then
      candidate_records+=("$record")
    fi
  done

  local index=1 built_count=0 accepted_count=0 skipped_claimed=0 skipped_small=0 fallback_notice_printed=false
  local -a built_tx_cbors=()
  local -a built_response_files=()
  local -a built_tx_id_claim_dirs=()
  local -a built_candidate_claim_dirs=()
  for record in "${candidate_records[@]}"; do
    [[ "$built_count" -ge "$TX_GENERATED_COUNT" ]] && break
    IFS=$'\t' read -r tx_in lovelace <<<"$record"
    if [[ "$lovelace" -lt "$min_spendable_lovelace" ]]; then
      skipped_small=$((skipped_small + 1))
      continue
    fi
    if [[ "$lovelace" -lt "$preferred_lovelace" && "$fallback_notice_printed" == false ]]; then
      if [[ "$has_preferred" == true ]]; then
        echo "[submit-tx] preferred UTxOs were unavailable or failed; falling back to spendable smaller inputs"
      else
        echo "[submit-tx] no UTxO reached the preferred $preferred_lovelace lovelace threshold; falling back to one spendable tx from the smallest available input"
      fi
      fallback_notice_printed=true
    fi

    candidate_claim_dir="$claim_dir/$(safe_claim_name "$tx_in")"
    if ! claim_tx_in "$tx_in" "$candidate_claim_dir"; then
      skipped_claimed=$((skipped_claimed + 1))
      continue
    fi

    local tx_body="$tx_dir/tx-$index.body"
    local tx_json="$tx_dir/tx-$index.json"
    local tx_cbor="$tx_dir/tx-$index.cbor"
    local response_file="$tx_dir/last-response-$index.txt"
    local output_lovelace="$TX_OUTPUT_LOVELACE"
    local tx_id tx_id_claim_dir

    echo "[submit-tx] building transaction $index from $tx_in..."
    if tx_query_uses_koios || ((lovelace < preferred_lovelace)); then
      if ! build_drain_transaction "$tx_in" "$lovelace" "$address" "$tx_body" "$protocol_params_file"; then
        echo "[submit-tx] failed to build drain transaction $index from $tx_in; releasing claim and continuing"
        release_tx_claim "$candidate_claim_dir"
        index=$((index + 1))
        continue
      fi
    else
      if ! "$CARDANO_CLI" conway transaction build \
        $(cardano_cli_network_args) \
        --socket-path "$socket" \
        --tx-in "$tx_in" \
        --tx-out "$address+$output_lovelace" \
        --change-address "$address" \
        $(tx_metadata_args) \
        --out-file "$tx_body"; then
        echo "[submit-tx] balanced build failed for $tx_in; trying drain transaction"
        if ! build_drain_transaction "$tx_in" "$lovelace" "$address" "$tx_body" "$protocol_params_file"; then
          echo "[submit-tx] failed to build transaction $index from $tx_in; releasing claim and continuing"
          release_tx_claim "$candidate_claim_dir"
          index=$((index + 1))
          continue
        fi
      fi
    fi

    if ! "$CARDANO_CLI" conway transaction sign \
      $(cardano_cli_network_args) \
      --tx-body-file "$tx_body" \
      --signing-key-file "$TX_PAYMENT_SKEY" \
      --out-canonical-cbor \
      --out-file "$tx_json"; then
      echo "[submit-tx] failed to sign transaction $index from $tx_in; releasing claim and continuing"
      release_tx_claim "$candidate_claim_dir"
      index=$((index + 1))
      continue
    fi

    if ! jq -r '.cborHex' "$tx_json" | xxd -r -p >"$tx_cbor"; then
      echo "[submit-tx] failed to extract canonical CBOR for transaction $index from $tx_in; releasing claim and continuing"
      release_tx_claim "$candidate_claim_dir"
      index=$((index + 1))
      continue
    fi

    if ! tx_id="$("$CARDANO_CLI" conway transaction txid --tx-file "$tx_json" --output-text)"; then
      echo "[submit-tx] failed to compute transaction id for transaction $index from $tx_in; releasing claim and continuing"
      release_tx_claim "$candidate_claim_dir"
      index=$((index + 1))
      continue
    fi
    tx_id_claim_dir="$tx_id_dir/$(safe_claim_name "$tx_id")"
    if ! claim_tx_id "$tx_id" "$tx_id_claim_dir" "$tx_in"; then
      echo "[submit-tx] skipping transaction $index tx_id=$tx_id because it was already built or submitted"
      release_tx_claim "$candidate_claim_dir"
      index=$((index + 1))
      continue
    fi

    echo "[submit-tx] built transaction $index tx_id=$tx_id"
    built_tx_cbors+=("$tx_cbor")
    built_response_files+=("$response_file")
    built_tx_id_claim_dirs+=("$tx_id_claim_dir")
    built_candidate_claim_dirs+=("$candidate_claim_dir")
    built_count=$((built_count + 1))
    index=$((index + 1))
  done

  if ((skipped_claimed > 0 || skipped_small > 0)); then
    echo "[submit-tx] ignored UTxOs while selecting inputs: already_claimed=$skipped_claimed below_minimum=$skipped_small minimum_spendable=$min_spendable_lovelace"
  fi

  if ((built_count > 0)); then
    wait_for_downstream_slot "$input_available_slot"
  fi

  local -a submit_pids=()
  local -a submit_result_files=()
  local submit_index submit_result_file tx_cbor response_file
  for submit_index in "${!built_tx_cbors[@]}"; do
    tx_cbor="${built_tx_cbors[$submit_index]}"
    response_file="${built_response_files[$submit_index]}"
    submit_result_file="$tx_dir/submit-result-$((submit_index + 1)).txt"
    submit_result_files+=("$submit_result_file")
    rm -f "$submit_result_file"
    (
      if submit_tx_with_retry "$tx_cbor" "$response_file"; then
        printf '%s\n' accepted >"$submit_result_file"
      else
        printf '%s\n' rejected >"$submit_result_file"
      fi
    ) &
    submit_pids+=("$!")
  done

  local submit_pid
  for submit_pid in "${submit_pids[@]}"; do
    wait "$submit_pid" || true
  done

  local submit_result tx_id_claim_dir
  for submit_index in "${!built_tx_cbors[@]}"; do
    tx_cbor="${built_tx_cbors[$submit_index]}"
    submit_result="$(cat "${submit_result_files[$submit_index]}" 2>/dev/null || true)"
    tx_id_claim_dir="${built_tx_id_claim_dirs[$submit_index]}"
    candidate_claim_dir="${built_candidate_claim_dirs[$submit_index]}"
    if [[ "$submit_result" == accepted ]]; then
      accept_tx_id_claim "$tx_id_claim_dir"
      accepted_count=$((accepted_count + 1))
      echo "[submit-tx] accepted $tx_cbor; keeping claim for this run"
    else
      echo "[submit-tx] tx $tx_cbor was not accepted; releasing claim and continuing with remaining transactions"
      release_tx_claim "$candidate_claim_dir"
      release_tx_claim "$tx_id_claim_dir"
    fi
  done

  if [[ "$accepted_count" -lt "$TX_GENERATED_COUNT" ]]; then
    echo "[submit-tx] accepted $accepted_count/$TX_GENERATED_COUNT requested transactions; not enough unclaimed spendable UTxOs were successfully submitted"
    return 1
  fi
}

# Submits several independent transactions in one run: submit_tx_batch [count]. The count comes
# from the argument, then TX_BATCH_COUNT, then an interactive prompt. Under process-compose there
# is no usable terminal to prompt on, so the batch process there is driven by TX_BATCH_COUNT and
# falls back to TX_BATCH_DEFAULT_COUNT rather than waiting on input nobody can give it.
submit_tx_batch() {
  local count="${1:-${TX_BATCH_COUNT:-}}"
  if [[ -z "$count" ]]; then
    if [[ -t 0 ]]; then
      printf '[submit-tx] how many independent transactions should be submitted? '
      read -r count || die "could not read transaction count"
    else
      count="${TX_BATCH_DEFAULT_COUNT:-5}"
      echo "[submit-tx] no TX_BATCH_COUNT and no terminal to prompt on; submitting $count transactions"
    fi
  fi

  [[ "$count" =~ ^[1-9][0-9]*$ ]] || die "transaction count must be a positive integer, got: $count"
  echo "[submit-tx] batch of $count independent transactions"
  TX_GENERATED_COUNT="$count"
  generate_submit
}

prepare_wallet() {
  require_cardano_cli
  [[ -n "$CARDANO_NODE_CONFIG_DIR" ]] || die "CARDANO_NODE_CONFIG_DIR must be set"
  [[ -n "$TX_PAYMENT_SKEY" ]] || die "TX_PAYMENT_SKEY must be set to a funded payment signing key"
  payment_skey_is_inline || [[ -f "$TX_PAYMENT_SKEY" ]] ||
    die "TX_PAYMENT_SKEY is neither a readable file nor key material: $TX_PAYMENT_SKEY"
  have jq || die "jq is required to query and select UTxOs"

  local socket address tx_dir utxo_file protocol_params_file tx_body tx_signed tx_cbor submit_error_file tx_id target_lovelace min_total_lovelace
  socket="$(cardano_node_socket_file)"
  tx_dir="$RUNDIR/generated/prepare-wallet"
  utxo_file="$tx_dir/utxo.json"
  protocol_params_file="$tx_dir/protocol-params.json"
  tx_body="$tx_dir/prepare.body"
  tx_signed="$tx_dir/prepare.signed"
  tx_cbor="$tx_dir/prepare.cbor"
  submit_error_file="$tx_dir/prepare-submit.err"
  target_lovelace=$((TX_REFUEL_UTXO_COUNT * TX_REFUEL_OUTPUT_LOVELACE))
  min_total_lovelace=$((target_lovelace + TX_FEE_BUFFER_LOVELACE))

  mkdir -p "$LOGDIR"
  exec > >(tee -a "$LOGDIR/prepare-wallet.log") 2>&1

  mkdir -p "$tx_dir"
  rm -f "$tx_dir"/* 2>/dev/null || true

  TX_PAYMENT_SKEY="$(resolve_payment_skey "$tx_dir")"
  prepare_tx_metadata "$tx_dir"
  address="$(payment_address "$tx_dir/payment.vkey")"

  # A single-transaction demo needs no distinct inputs, and preparing the wallet would spend a fee
  # to split a UTxO that submit-tx can already drain as it is.
  if ((TX_REFUEL_UTXO_COUNT == 0)); then
    echo "[prepare-wallet] TX_REFUEL_UTXO_COUNT=0; skipping wallet preparation"
    clear_submit_claim_state
    echo "[prepare-wallet] cleared submit-tx claim state"
    return 0
  fi

  if tx_query_uses_koios; then
    echo "[prepare-wallet] using Koios at $KOIOS_API_URL as the upstream query source"
  else
    echo "[prepare-wallet] waiting for the upstream cardano-node socket to answer local queries and reach ${TX_REFUEL_CARDANO_SYNC_PROGRESS}% sync..."
  fi
  wait_for_upstream_ready

  echo "[prepare-wallet] using payment address: $address"
  query_address_utxo "$socket" "$address" "$utxo_file" || die "could not query the UTxOs at $address"

  local clean_count spendable_count
  spendable_count="$(count_spendable_outputs "$utxo_file")"
  if ((spendable_count >= TX_REFUEL_UTXO_COUNT)) && ! truthy "$TX_REFUEL_FORCE"; then
    echo "[prepare-wallet] ready: $spendable_count outputs are already spendable by submit-tx, no preparation transaction needed; set TX_REFUEL_FORCE=true to rebuild them anyway"
    clear_submit_claim_state
    echo "[prepare-wallet] cleared submit-tx claim state"
    return 0
  fi
  if truthy "$TX_REFUEL_FORCE"; then
    echo "[prepare-wallet] TX_REFUEL_FORCE=true; rebuilding even though $spendable_count outputs are already spendable"
  else
    echo "[prepare-wallet] only $spendable_count of ${TX_REFUEL_UTXO_COUNT} needed outputs are spendable; preparing the wallet"
  fi

  local -a selected_tx_ins=()
  local total_lovelace=0 input_count=0 record tx_in lovelace
  local sort_args=()
  case "$TX_REFUEL_SELECTION" in
  largest) sort_args=(-k2,2nr) ;;
  smallest) sort_args=(-k2,2n) ;;
  *) die "TX_REFUEL_SELECTION must be 'largest' or 'smallest', got '$TX_REFUEL_SELECTION'" ;;
  esac

  while IFS= read -r record; do
    IFS=$'\t' read -r tx_in lovelace <<<"$record"
    selected_tx_ins+=("$tx_in")
    total_lovelace=$((total_lovelace + lovelace))
    input_count=$((input_count + 1))
    if ((total_lovelace >= min_total_lovelace)); then
      break
    fi
    if ((input_count >= TX_REFUEL_MAX_INPUTS)); then
      break
    fi
  done < <(
    jq -r '
      to_entries[]
      | [.key, (.value.value.lovelace // 0)]
      | @tsv
    ' "$utxo_file" | sort "${sort_args[@]}"
  )

  # Wallet preparation is an optimization for concurrent submitters, not a precondition for
  # submitting at all, so a wallet that cannot afford the clean outputs leaves the existing UTxOs
  # alone and lets submit-tx work with them. Failing here would instead block every process that
  # waits for this one to complete.
  if ((input_count == 0)); then
    echo "[prepare-wallet] no UTxO found for $address; fund the address to submit transactions"
    clear_submit_claim_state
    return 0
  fi
  if ((total_lovelace < min_total_lovelace)); then
    echo "[prepare-wallet] $total_lovelace lovelace from $input_count inputs cannot fund ${TX_REFUEL_UTXO_COUNT}x${TX_REFUEL_OUTPUT_LOVELACE} lovelace plus the $TX_FEE_BUFFER_LOVELACE fee buffer; leaving the wallet as it is"
    echo "[prepare-wallet] lower TX_REFUEL_UTXO_COUNT or TX_REFUEL_OUTPUT_LOVELACE to prepare clean inputs for concurrent submitters"
    clear_submit_claim_state
    return 0
  fi

  # Inputs and outputs are kept apart because the two build paths need different things: the local
  # node balances a complete transaction, while build_refuel_transaction derives the outputs itself
  # from TX_REFUEL_UTXO_COUNT and must be given the inputs alone.
  local -a tx_in_args=() tx_out_args=()
  for tx_in in "${selected_tx_ins[@]}"; do
    tx_in_args+=(--tx-in "$tx_in")
  done
  for ((i = 0; i < TX_REFUEL_UTXO_COUNT; i++)); do
    tx_out_args+=(--tx-out "$address+$TX_REFUEL_OUTPUT_LOVELACE")
  done

  echo "[prepare-wallet] building preparation transaction from $input_count inputs totaling $total_lovelace lovelace into ${TX_REFUEL_UTXO_COUNT} outputs of $TX_REFUEL_OUTPUT_LOVELACE lovelace..."
  if tx_query_uses_koios; then
    # No node to balance the transaction, so the fee and change are calculated here.
    query_koios_protocol_parameters "$protocol_params_file"
    build_refuel_transaction "$total_lovelace" "$address" "$tx_body" "$protocol_params_file" "${tx_in_args[@]}" ||
      die "could not build the preparation transaction"
  else
    "$CARDANO_CLI" conway transaction build \
      $(cardano_cli_network_args) \
      --socket-path "$socket" \
      "${tx_in_args[@]}" "${tx_out_args[@]}" \
      --change-address "$address" \
      $(tx_metadata_args) \
      --out-file "$tx_body"
  fi

  # Koios takes the raw CBOR bytes, so sign canonically and extract them; the node socket path
  # submits the cardano-cli JSON directly.
  "$CARDANO_CLI" conway transaction sign \
    $(cardano_cli_network_args) \
    --tx-body-file "$tx_body" \
    --signing-key-file "$TX_PAYMENT_SKEY" \
    --out-canonical-cbor \
    --out-file "$tx_signed"
  jq -r '.cborHex' "$tx_signed" | xxd -r -p >"$tx_cbor"

  tx_id="$("$CARDANO_CLI" conway transaction txid --tx-file "$tx_signed" --output-text)"
  echo "[prepare-wallet] submitting preparation transaction tx_id=$tx_id"
  if ! submit_tx_upstream "$socket" "$tx_signed" "$tx_cbor" "$submit_error_file"; then
    if rg -qi 'All inputs are spent|BadInputsUTxO' "$submit_error_file"; then
      cat "$submit_error_file"
      echo "[prepare-wallet] submit input is already spent; continuing as tx_id=$tx_id may already be included"
    else
      cat "$submit_error_file"
      return 1
    fi
  fi

  clear_submit_claim_state
  echo "[prepare-wallet] cleared submit-tx claim state"
  echo "[prepare-wallet] waiting for ${TX_REFUEL_UTXO_COUNT} clean outputs from tx_id=$tx_id..."

  local deadline=$((SECONDS + TX_REFUEL_CONFIRM_TIMEOUT_SECONDS))
  while ((SECONDS < deadline)); do
    query_address_utxo "$socket" "$address" "$utxo_file" || true
    clean_count="$(count_clean_refuel_outputs "$utxo_file")"
    if ((clean_count >= TX_REFUEL_UTXO_COUNT)); then
      echo "[prepare-wallet] ready: found $clean_count clean outputs after tx_id=$tx_id"
      return 0
    fi
    echo "[prepare-wallet] found $clean_count/${TX_REFUEL_UTXO_COUNT} clean outputs; waiting..."
    sleep 5
  done

  die "timed out waiting for prepared wallet outputs from tx_id=$tx_id; the transaction may still confirm later"
}
