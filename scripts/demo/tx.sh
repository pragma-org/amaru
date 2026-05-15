#!/usr/bin/env bash

# Transaction configuration. Callers may override these with environment variables.
TX_PAYMENT_SKEY="${TX_PAYMENT_SKEY:-}"
TX_GENERATED_COUNT="${TX_GENERATED_COUNT:-1}"
TX_SYNC_SLOT_TOLERANCE="${TX_SYNC_SLOT_TOLERANCE:-100}"
TX_SYNC_TIMEOUT_SECONDS="${TX_SYNC_TIMEOUT_SECONDS:-14400}"
TX_SUBMIT_RETRY_LIMIT="${TX_SUBMIT_RETRY_LIMIT:-12}"
TX_SUBMIT_RETRY_DELAY="${TX_SUBMIT_RETRY_DELAY:-30}"
TX_WAIT_FOR_SYNC="${TX_WAIT_FOR_SYNC:-true}"
TX_OUTPUT_LOVELACE="${TX_OUTPUT_LOVELACE:-1000000}"
TX_FEE_BUFFER_LOVELACE="${TX_FEE_BUFFER_LOVELACE:-300000}"
TX_CHANGE_BUFFER_LOVELACE="${TX_CHANGE_BUFFER_LOVELACE:-900000}"

# Returns whether runtime transaction generation is configured.
tx_generation_enabled() {
  [[ -n "$TX_PAYMENT_SKEY" ]]
}

# Validates optional runtime transaction generation settings before startup.
require_configured_tx() {
  if tx_generation_enabled; then
    require_cardano_cli
    have jq || die "jq is required to generate transactions"
    have curl || die "curl not found"
    [[ -f "$TX_PAYMENT_SKEY" ]] || die "TX_PAYMENT_SKEY does not exist: $TX_PAYMENT_SKEY"
  fi
}

# Submits a transaction to the downstream submit API with retryable rejection handling.
submit_tx_with_retry() {
  local tx_file="$1" response_file="$2" attempt="${3:-1}"
  local response status body retries="$TX_SUBMIT_RETRY_LIMIT" delay="$TX_SUBMIT_RETRY_DELAY"
  while (( attempt <= retries )); do
    echo "[submit-tx] attempt $attempt/$retries: submitting $tx_file to downstream Amaru at $DOWNSTREAM_SUBMIT_API_ADDRESS"
    if curl -sS -o "$response_file" -w '%{http_code}' \
        -X POST -H 'Content-Type: application/cbor' \
        --data-binary "@$tx_file" \
        "http://$DOWNSTREAM_SUBMIT_API_ADDRESS/api/submit/tx" > "$response_file.status"; then
      status="$(cat "$response_file.status")"
      body="$(cat "$response_file")"
      echo "[submit-tx] response: HTTP $status"
      echo "$body"
      if [[ "$status" == 2* ]]; then
        return 0
      fi
      if echo "$body" | grep -qi 'missing transaction inputs'; then
        echo "[submit-tx] downstream ledger does not yet contain the input UTxO; waiting ${delay}s before retry"
      else
        echo "[submit-tx] non-retryable rejection; aborting retries for this tx"
        return 1
      fi
    else
      echo "[submit-tx] curl failed; will retry in ${delay}s"
    fi
    sleep "$delay"
    attempt=$(( attempt + 1 ))
  done
  echo "[submit-tx] giving up after $retries attempts"
  return 1
}

# Derives the payment address used for generated transactions.
payment_address() {
  local magic="$1" vkey="${2:-$RUNDIR/generated/payment.vkey}"

  "$CARDANO_CLI" conway key verification-key --signing-key-file "$TX_PAYMENT_SKEY" --verification-key-file "$vkey" >/dev/null
  "$CARDANO_CLI" conway address build --payment-verification-key-file "$vkey" --testnet-magic "$magic"
}

safe_claim_name() {
  local tx_in="$1"
  echo "${tx_in//[^A-Za-z0-9]/_}"
}

claim_tx_in() {
  local tx_in="$1" claim_dir="$2"
  if mkdir "$claim_dir" 2>/dev/null; then
    printf '%s\n' "$tx_in" > "$claim_dir/tx-in"
    printf '%s\n' "$$" > "$claim_dir/pid"
    return 0
  fi
  return 1
}

release_tx_claim() {
  local claim_dir="$1"
  rm -rf "$claim_dir"
}

# Waits for Amaru nodes to sync and writes progress to the submit transaction log.
wait_sync() {
  require_cardano_cli
  [[ -n "$CARDANO_NODE_CONFIG_DIR" ]] || die "CARDANO_NODE_CONFIG_DIR must be set"
  have jq || die "jq is required to query cardano-node tip"
  mkdir -p "$LOGDIR"
  exec > >(tee -a "$LOGDIR/submit-tx.log") 2>&1
  wait_for_amaru_sync_to_cardano_node
}

wait_before_submit() {
  local input_available_slot="$1"
  case "$TX_WAIT_FOR_SYNC" in
    1 | true | TRUE | yes | YES | on | ON)
      wait_for_downstream_slot "$input_available_slot"
      ;;
    full | FULL)
      wait_for_amaru_sync_to_cardano_node
      ;;
    *)
      echo "[submit-tx] TX_WAIT_FOR_SYNC=$TX_WAIT_FOR_SYNC; submitting without waiting for Amaru catch-up"
      ;;
  esac
}

# Generates transactions from configured payment credentials and submits them.
generate_submit() {
  require_cardano_cli
  [[ -n "$CARDANO_NODE_CONFIG_DIR" ]] || die "CARDANO_NODE_CONFIG_DIR must be set"
  [[ -n "$TX_PAYMENT_SKEY" ]] || die "TX_PAYMENT_SKEY must be set to a funded preprod payment signing key"
  [[ -f "$TX_PAYMENT_SKEY" ]] || die "TX_PAYMENT_SKEY does not exist: $TX_PAYMENT_SKEY"
  have jq || die "jq is required to query and select UTxOs"
  have curl || die "curl not found"

  local socket replica_num
  socket="$(cardano_node_socket_file)"
  replica_num="${PC_REPLICA_NUM:-0}"
  local magic address tx_dir claim_dir utxo_file preferred_lovelace min_spendable_lovelace input_available_slot
  magic="$(network_magic)"
  tx_dir="$RUNDIR/generated/submit-tx-$replica_num"
  claim_dir="$RUNDIR/generated/submit-tx-claims"
  utxo_file="$tx_dir/utxo.json"
  preferred_lovelace=3000000
  min_spendable_lovelace=$((TX_OUTPUT_LOVELACE + TX_FEE_BUFFER_LOVELACE + TX_CHANGE_BUFFER_LOVELACE))

  mkdir -p "$tx_dir" "$claim_dir" "$LOGDIR"
  exec > >(tee -a "$LOGDIR/submit-tx.log") 2>&1

  address="$(payment_address "$magic" "$tx_dir/payment.vkey")"
  echo "[submit-tx] waiting for upstream cardano-node socket to answer local queries..."
  wait_for_cardano_socket
  wait_for_cardano_query "$magic"

  echo "[submit-tx] using payment address: $address"
  echo "[submit-tx] querying UTxO from upstream cardano-node socket..."
  local queried=false
  for _ in {1..60}; do
    if "$CARDANO_CLI" conway query utxo \
      --testnet-magic "$magic" \
      --socket-path "$socket" \
      --address "$address" \
      --output-json \
      --out-file "$utxo_file"; then
      queried=true
      break
    fi
    sleep 1
  done
  [[ "$queried" == true ]] || die "could not query UTxO from cardano-node socket: $socket"
  input_available_slot="$(cardano_node_tip_slot "$magic" || true)"
  [[ -n "$input_available_slot" ]] || die "could not query cardano-node tip after selecting UTxOs"
  echo "[submit-tx] selected UTxO set is available at cardano-node slot $input_available_slot"

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

  local -a selected_tx_ins=()
  local -a selected_claim_dirs=()
  local record tx_in lovelace candidate_claim_dir
  for record in "${tx_records[@]}"; do
    IFS=$'\t' read -r tx_in lovelace <<< "$record"
    if [[ "$lovelace" -ge "$preferred_lovelace" ]]; then
      candidate_claim_dir="$claim_dir/$(safe_claim_name "$tx_in")"
      if claim_tx_in "$tx_in" "$candidate_claim_dir"; then
        selected_tx_ins+=("$tx_in")
        selected_claim_dirs+=("$candidate_claim_dir")
      else
        echo "[submit-tx] skipping already claimed UTxO $tx_in"
      fi
      [[ ${#selected_tx_ins[@]} -ge "$TX_GENERATED_COUNT" ]] && break
    fi
  done

  if [[ ${#selected_tx_ins[@]} -eq 0 ]]; then
    for record in "${tx_records[@]}"; do
      IFS=$'\t' read -r tx_in lovelace <<< "$record"
      if [[ "$lovelace" -lt "$min_spendable_lovelace" ]]; then
        echo "[submit-tx] skipping UTxO $tx_in: $lovelace lovelace is below the minimum spendable $min_spendable_lovelace lovelace"
        continue
      fi
      candidate_claim_dir="$claim_dir/$(safe_claim_name "$tx_in")"
      if claim_tx_in "$tx_in" "$candidate_claim_dir"; then
        selected_tx_ins=("$tx_in")
        selected_claim_dirs=("$candidate_claim_dir")
        echo "[submit-tx] no UTxO reached the preferred $preferred_lovelace lovelace threshold; falling back to one spendable tx from the smallest available input"
        break
      fi
      echo "[submit-tx] skipping already claimed UTxO $tx_in"
    done
  fi

  if [[ ${#selected_tx_ins[@]} -eq 0 ]]; then
    echo "[submit-tx] no unclaimed spendable UTxO found for $address; fund UTxOs larger than $min_spendable_lovelace lovelace or restart the demo to clear stale submit-tx claims"
    echo "[submit-tx] nothing to submit from this replica"
    return 0
  fi

  wait_before_submit "$input_available_slot"

  local index=1 claim_index=0
  local tx_files=()
  local tx_claim_dirs=()
  for tx_in in "${selected_tx_ins[@]}"; do
    local tx_body="$tx_dir/tx-$index.body"
    local tx_json="$tx_dir/tx-$index.json"
    local tx_cbor="$tx_dir/tx-$index.cbor"
    local output_lovelace="$TX_OUTPUT_LOVELACE"

    echo "[submit-tx] building transaction $index from $tx_in..."
    if ! "$CARDANO_CLI" conway transaction build \
      --testnet-magic "$magic" \
      --socket-path "$socket" \
      --tx-in "$tx_in" \
      --tx-out "$address+$output_lovelace" \
      --change-address "$address" \
      --out-file "$tx_body"; then
      echo "[submit-tx] failed to build transaction $index from $tx_in; releasing claim and continuing"
      release_tx_claim "${selected_claim_dirs[$claim_index]}"
      index=$((index + 1))
      claim_index=$((claim_index + 1))
      continue
    fi

    if ! "$CARDANO_CLI" conway transaction sign \
      --testnet-magic "$magic" \
      --tx-body-file "$tx_body" \
      --signing-key-file "$TX_PAYMENT_SKEY" \
      --out-canonical-cbor \
      --out-file "$tx_json"; then
      echo "[submit-tx] failed to sign transaction $index from $tx_in; releasing claim and continuing"
      release_tx_claim "${selected_claim_dirs[$claim_index]}"
      index=$((index + 1))
      claim_index=$((claim_index + 1))
      continue
    fi

    if ! jq -r '.cborHex' "$tx_json" | xxd -r -p > "$tx_cbor"; then
      echo "[submit-tx] failed to extract canonical CBOR for transaction $index from $tx_in; releasing claim and continuing"
      release_tx_claim "${selected_claim_dirs[$claim_index]}"
      index=$((index + 1))
      claim_index=$((claim_index + 1))
      continue
    fi

    tx_files+=("$tx_cbor")
    tx_claim_dirs+=("${selected_claim_dirs[$claim_index]}")
    index=$((index + 1))
    claim_index=$((claim_index + 1))
  done

  [[ ${#tx_files[@]} -gt 0 ]] || die "no transactions were built; check UTxO sizes and transaction build errors above"

  local tx_file tx_claim_dir response_file
  index=1
  for tx_file in "${tx_files[@]}"; do
    tx_claim_dir="${tx_claim_dirs[$((index - 1))]}"
    response_file="$tx_dir/last-response-$index.txt"
    if submit_tx_with_retry "$tx_file" "$response_file"; then
      echo "[submit-tx] accepted $tx_file; keeping claim for this run"
    else
      echo "[submit-tx] tx $tx_file was not accepted; releasing claim and continuing with remaining transactions"
      release_tx_claim "$tx_claim_dir"
    fi
    index=$((index + 1))
  done
}
