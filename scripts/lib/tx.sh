#!/usr/bin/env bash

# Transaction configuration. Callers may override these with environment variables.
TX_PAYMENT_SKEY="${TX_PAYMENT_SKEY:-}"
TX_SUBMIT_HTTP_TIMEOUT_SECONDS="${TX_SUBMIT_HTTP_TIMEOUT_SECONDS:-30}"
TX_SUBMIT_RETRY_LIMIT="${TX_SUBMIT_RETRY_LIMIT:-12}"
TX_SUBMIT_RETRY_DELAY="${TX_SUBMIT_RETRY_DELAY:-30}"
TX_OUTPUT_LOVELACE="${TX_OUTPUT_LOVELACE:-1000000}"
TX_FEE_BUFFER_LOVELACE="${TX_FEE_BUFFER_LOVELACE:-300000}"
TX_METADATA_MESSAGE="${TX_METADATA_MESSAGE-made by amaru with 💜}"

# CIP-20 defines label 674 with a `msg` array for human-readable transaction messages, which is what
# explorers render as a transaction comment.
TX_METADATA_LABEL=674

# The ledger rejects any metadata text string longer than this, counted in bytes: an emoji costs
# several, so the limit bites well before the message looks long.
TX_METADATA_MAX_BYTES=64

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
    # the wallet funds, so the rejection has to happen here.
    [[ "$(payment_skey_format)" != "cardano-address-root" ]] || die "TX_PAYMENT_SKEY is a cardano-address root key: $TX_PAYMENT_SKEY; derive the payment key (addr_xsk) from it first"
    local message_bytes
    message_bytes=$(printf '%s' "$TX_METADATA_MESSAGE" | wc -c | tr -d ' ')
    ((message_bytes <= TX_METADATA_MAX_BYTES)) ||
      die "TX_METADATA_MESSAGE is $message_bytes bytes, over the $TX_METADATA_MAX_BYTES the ledger allows for one metadata string: '$TX_METADATA_MESSAGE'"
  fi
}

submit_tx_response_is_duplicate() {
  grep -qi 'transaction is a duplicate' <<<"$1"
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

submit_tx_accept_expected_id() {
  local status="$1" response_file="$2" expected_tx_id="$3"

  [[ "$status" == 202 ]] || return 1
  if submit_tx_response_matches_id "$expected_tx_id" "$response_file"; then
    return 0
  fi
  echo "[submit-tx] HTTP 202 response did not contain the expected transaction id"
  return 2
}

submit_tx_with_retry_policy() {
  local tx_file="$1" response_file="$2" accept_response="$3" expected_tx_id="${4:-}" attempt=1
  local status body acceptance_status retries="$TX_SUBMIT_RETRY_LIMIT" delay="$TX_SUBMIT_RETRY_DELAY" subject="$tx_file"

  [[ -z "$expected_tx_id" ]] || subject="tx_id=$expected_tx_id"
  while ((attempt <= retries)); do
    echo "[submit-tx] attempt $attempt/$retries: submitting $subject to the Amaru submit API at $TX_SUBMIT_API_ADDRESS"
    if curl --max-time "$TX_SUBMIT_HTTP_TIMEOUT_SECONDS" -sS -o "$response_file" -w '%{http_code}' \
      -X POST -H 'Content-Type: application/cbor' \
      --data-binary "@$tx_file" \
      "http://$TX_SUBMIT_API_ADDRESS/api/submit/tx" >"$response_file.status"; then
      status="$(cat "$response_file.status")"
      body="$(cat "$response_file")"
      echo "[submit-tx] response: HTTP $status"
      echo "$body"
      if "$accept_response" "$status" "$response_file" "$expected_tx_id"; then
        return 0
      else
        acceptance_status=$?
      fi
      if ((acceptance_status != 1)); then
        return 1
      fi
      if submit_tx_response_is_duplicate "$body"; then
        echo "[submit-tx] downstream already knows this transaction; submission is complete"
        return 0
      fi
      if grep -qi 'missing transaction inputs' <<<"$body"; then
        echo "[submit-tx] Amaru has not indexed the input UTxO yet; waiting ${delay}s before retry"
      else
        echo "[submit-tx] non-retryable HTTP $status rejection; aborting retries for this transaction"
        return 1
      fi
    else
      echo "[submit-tx] request failed; waiting ${delay}s before retry"
    fi
    sleep "$delay"
    attempt=$((attempt + 1))
  done
  echo "[submit-tx] giving up after $retries attempts"
  return 1
}

submit_tx_and_expect_id() {
  submit_tx_with_retry_policy "$1" "$3" submit_tx_accept_expected_id "$2"
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
# signing key, so the script can only ever build transactions it can also sign.
payment_address() {
  local vkey="$1"

  "$CARDANO_CLI" conway key verification-key --signing-key-file "$TX_PAYMENT_SKEY" --verification-key-file "$vkey" >/dev/null
  "$CARDANO_CLI" conway address build --payment-verification-key-file "$vkey" $(cardano_cli_network_args)
}

# Queries the UTxOs at an address from Koios, retrying because a public endpoint under load answers
# with a non-JSON error page.
query_address_utxo() {
  local address="$1" utxo_file="$2" attempt

  for attempt in {1..60}; do
    if query_koios_payment_utxo "$address" "$utxo_file"; then
      return 0
    fi
    echo "[tx] Koios UTxO query failed (attempt $attempt); retrying..."
    sleep 5
  done
  return 1
}

# Makes sure that Koios answers, since it only ever serves the real tip.
wait_for_upstream_ready() {
  [[ -n "$(koios_tip_slot || true)" ]] || die "could not reach Koios at $KOIOS_API_URL"
}

koios_tip_slot() {
  curl --max-time "${KOIOS_TIMEOUT_SECONDS:-30}" -fsSL \
    -H 'accept: application/json' \
    "$KOIOS_API_URL/tip" | jq -r '.[0].abs_slot // empty'
}

# Fetches the current protocol parameters from Koios in cardano-cli format.
query_upstream_protocol_parameters() {
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
