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
TX_DRAIN_SMALL_UTXOS="${TX_DRAIN_SMALL_UTXOS:-true}"
TX_REFUEL_UTXO_COUNT="${TX_REFUEL_UTXO_COUNT:-10}"
TX_REFUEL_OUTPUT_LOVELACE="${TX_REFUEL_OUTPUT_LOVELACE:-2000000}"
TX_REFUEL_MAX_INPUTS="${TX_REFUEL_MAX_INPUTS:-80}"
TX_REFUEL_CONFIRM_TIMEOUT_SECONDS="${TX_REFUEL_CONFIRM_TIMEOUT_SECONDS:-300}"
TX_REFUEL_SELECTION="${TX_REFUEL_SELECTION:-largest}"
TX_REFUEL_FORCE="${TX_REFUEL_FORCE:-false}"

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
      if echo "$body" | grep -qi 'transaction is a duplicate'; then
        echo "[submit-tx] downstream already knows this transaction; keeping claim"
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

query_protocol_parameters() {
  local magic="$1" socket="$2" protocol_params_file="$3"
  "$CARDANO_CLI" conway query protocol-parameters \
    --testnet-magic "$magic" \
    --socket-path "$socket" \
    --out-file "$protocol_params_file"
}

query_payment_utxo() {
  local magic="$1" socket="$2" address="$3" utxo_file="$4"
  "$CARDANO_CLI" conway query utxo \
    --testnet-magic "$magic" \
    --socket-path "$socket" \
    --address "$address" \
    --output-json \
    --out-file "$utxo_file"
}

build_drain_transaction() {
  local magic="$1" tx_in="$2" lovelace="$3" address="$4" tx_body="$5" protocol_params_file="$6"
  local draft_body="$tx_body.draft" fee output_lovelace

  "$CARDANO_CLI" conway transaction build-raw \
    --tx-in "$tx_in" \
    --tx-out "$address+$lovelace" \
    --fee 0 \
    --out-file "$draft_body"

  fee="$(
    "$CARDANO_CLI" conway transaction calculate-min-fee \
      --tx-body-file "$draft_body" \
      --protocol-params-file "$protocol_params_file" \
      --witness-count 1 \
      --testnet-magic "$magic" \
      --output-text \
      | awk '{print $1}'
  )"
  [[ "$fee" =~ ^[0-9]+$ ]] || return 1

  output_lovelace=$((lovelace - fee))
  if ((output_lovelace < TX_OUTPUT_LOVELACE)); then
    echo "[submit-tx] cannot drain $tx_in: output $output_lovelace lovelace would be below TX_OUTPUT_LOVELACE=$TX_OUTPUT_LOVELACE"
    return 1
  fi

  "$CARDANO_CLI" conway transaction build-raw \
    --tx-in "$tx_in" \
    --tx-out "$address+$output_lovelace" \
    --fee "$fee" \
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

claim_tx_id() {
  local tx_id="$1" claim_dir="$2" tx_in="$3"
  if mkdir "$claim_dir" 2>/dev/null; then
    printf '%s\n' "$tx_id" > "$claim_dir/tx-id"
    printf '%s\n' "$tx_in" > "$claim_dir/tx-in"
    printf '%s\n' "$$" > "$claim_dir/pid"
    printf '%s\n' "pending" > "$claim_dir/status"
    return 0
  fi
  return 1
}

accept_tx_id_claim() {
  local claim_dir="$1"
  printf '%s\n' "accepted" > "$claim_dir/status"
}

acquire_submit_claims_lock() {
  local lock_dir="$1" timeout=60 pid
  for (( elapsed = 0; elapsed < timeout; elapsed++ )); do
    if mkdir "$lock_dir" 2>/dev/null; then
      printf '%s\n' "$$" > "$lock_dir/pid"
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
  if truthy "$CLEAR_SUBMIT_TX_CLAIMS_ON_START" && ! find "$active_dir" -mindepth 1 -maxdepth 1 -type d | grep -q .; then
    echo "[submit-tx] clearing stale submit-tx claims from $claim_dir"
    rm -rf "$claim_dir"
  fi
  mkdir -p "$claim_dir" "$active_replica_dir"
  printf '%s\n' "$$" > "$active_replica_dir/pid"
  printf '%s\n' "$replica_num" > "$active_replica_dir/replica"
  rm -rf "$lock_dir"
  SUBMIT_TX_ACTIVE_REPLICA_DIR="$active_replica_dir"
}

release_submit_active_replica() {
  if [[ -n "${SUBMIT_TX_ACTIVE_REPLICA_DIR:-}" ]]; then
    rm -rf "$SUBMIT_TX_ACTIVE_REPLICA_DIR"
  fi
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
  [[ -n "$TX_PAYMENT_SKEY" ]] || die "TX_PAYMENT_SKEY must be set to a funded testnet payment signing key"
  [[ -f "$TX_PAYMENT_SKEY" ]] || die "TX_PAYMENT_SKEY does not exist: $TX_PAYMENT_SKEY"
  have jq || die "jq is required to query and select UTxOs"
  have curl || die "curl not found"

  local socket replica_num
  socket="$(cardano_node_socket_file)"
  replica_num="${PC_REPLICA_NUM:-0}"
  local magic address tx_dir claim_dir tx_id_dir claim_lock_dir active_dir utxo_file protocol_params_file preferred_lovelace min_spendable_lovelace input_available_slot
  magic="$(network_magic)"
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
  rm -f "$tx_dir"/tx-* "$tx_dir"/last-response-* "$tx_dir"/protocol-params.json "$tx_dir"/utxo.json "$tx_dir"/payment.vkey 2>/dev/null || true
  exec > >(tee -a "$LOGDIR/submit-tx.log") 2>&1
  validate_amaru_runtime_network "${AMARU_MIDDLE_LOG_FILE:-$LOGDIR/amaru-middle.log}" "middle"
  validate_amaru_runtime_network "${AMARU_DOWNSTREAM_LOG_FILE:-$LOGDIR/amaru-downstream.log}" "downstream"
  initialize_submit_claims "$claim_dir" "$tx_id_dir" "$claim_lock_dir" "$active_dir" "$replica_num"
  trap release_submit_active_replica EXIT

  address="$(payment_address "$magic" "$tx_dir/payment.vkey")"
  echo "[submit-tx] waiting for upstream cardano-node socket to answer local queries..."
  wait_for_cardano_socket
  wait_for_cardano_query "$magic"

  echo "[submit-tx] using payment address: $address"
  echo "[submit-tx] querying UTxO from upstream cardano-node socket..."
  local queried=false
  for _ in {1..60}; do
    if query_payment_utxo "$magic" "$socket" "$address" "$utxo_file"; then
      queried=true
      break
    fi
    sleep 1
  done
  [[ "$queried" == true ]] || die "could not query UTxO from cardano-node socket: $socket"
  query_protocol_parameters "$magic" "$socket" "$protocol_params_file"
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

  local -a candidate_records=()
  local has_preferred=false
  local record tx_in lovelace candidate_claim_dir
  for record in "${tx_records[@]}"; do
    IFS=$'\t' read -r tx_in lovelace <<< "$record"
    if [[ "$lovelace" -ge "$preferred_lovelace" ]]; then
      candidate_records+=("$record")
      has_preferred=true
    fi
  done
  for record in "${tx_records[@]}"; do
    IFS=$'\t' read -r tx_in lovelace <<< "$record"
    if [[ "$lovelace" -lt "$preferred_lovelace" ]]; then
      candidate_records+=("$record")
    fi
  done

  local index=1 accepted_count=0 skipped_claimed=0 skipped_small=0 fallback_notice_printed=false sync_waited=false
  for record in "${candidate_records[@]}"; do
    [[ "$accepted_count" -ge "$TX_GENERATED_COUNT" ]] && break
    IFS=$'\t' read -r tx_in lovelace <<< "$record"
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
    if truthy "$TX_DRAIN_SMALL_UTXOS" && (( lovelace < preferred_lovelace )); then
      if ! build_drain_transaction "$magic" "$tx_in" "$lovelace" "$address" "$tx_body" "$protocol_params_file"; then
        echo "[submit-tx] failed to build drain transaction $index from $tx_in; releasing claim and continuing"
        release_tx_claim "$candidate_claim_dir"
        index=$((index + 1))
        continue
      fi
    else
      if ! "$CARDANO_CLI" conway transaction build \
        --testnet-magic "$magic" \
        --socket-path "$socket" \
        --tx-in "$tx_in" \
        --tx-out "$address+$output_lovelace" \
        --change-address "$address" \
        --out-file "$tx_body"; then
        echo "[submit-tx] balanced build failed for $tx_in; trying drain transaction"
        if ! build_drain_transaction "$magic" "$tx_in" "$lovelace" "$address" "$tx_body" "$protocol_params_file"; then
          echo "[submit-tx] failed to build transaction $index from $tx_in; releasing claim and continuing"
          release_tx_claim "$candidate_claim_dir"
          index=$((index + 1))
          continue
        fi
      fi
    fi

    if ! "$CARDANO_CLI" conway transaction sign \
      --testnet-magic "$magic" \
      --tx-body-file "$tx_body" \
      --signing-key-file "$TX_PAYMENT_SKEY" \
      --out-canonical-cbor \
      --out-file "$tx_json"; then
      echo "[submit-tx] failed to sign transaction $index from $tx_in; releasing claim and continuing"
      release_tx_claim "$candidate_claim_dir"
      index=$((index + 1))
      continue
    fi

    if ! jq -r '.cborHex' "$tx_json" | xxd -r -p > "$tx_cbor"; then
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

    if [[ "$sync_waited" == false ]]; then
      wait_before_submit "$input_available_slot"
      sync_waited=true
    fi

    if submit_tx_with_retry "$tx_cbor" "$response_file"; then
      accept_tx_id_claim "$tx_id_claim_dir"
      accepted_count=$((accepted_count + 1))
      echo "[submit-tx] accepted $tx_cbor; keeping claim for this run"
    else
      echo "[submit-tx] tx $tx_cbor was not accepted; releasing claim and continuing with remaining transactions"
      release_tx_claim "$candidate_claim_dir"
      release_tx_claim "$tx_id_claim_dir"
    fi
    index=$((index + 1))
  done

  if ((skipped_claimed > 0 || skipped_small > 0)); then
    echo "[submit-tx] ignored UTxOs while selecting inputs: already_claimed=$skipped_claimed below_minimum=$skipped_small minimum_spendable=$min_spendable_lovelace"
  fi

  if [[ "$accepted_count" -lt "$TX_GENERATED_COUNT" ]]; then
    echo "[submit-tx] accepted $accepted_count/$TX_GENERATED_COUNT requested transactions; not enough unclaimed spendable UTxOs were successfully submitted"
    return 1
  fi
}

refuel_submit_wallet() {
  require_cardano_cli
  [[ -n "$CARDANO_NODE_CONFIG_DIR" ]] || die "CARDANO_NODE_CONFIG_DIR must be set"
  [[ -n "$TX_PAYMENT_SKEY" ]] || die "TX_PAYMENT_SKEY must be set to a funded testnet payment signing key"
  [[ -f "$TX_PAYMENT_SKEY" ]] || die "TX_PAYMENT_SKEY does not exist: $TX_PAYMENT_SKEY"
  have jq || die "jq is required to query and select UTxOs"

  local socket magic address tx_dir utxo_file tx_body tx_signed submit_error_file tx_id target_lovelace min_total_lovelace
  socket="$(cardano_node_socket_file)"
  magic="$(network_magic)"
  tx_dir="$RUNDIR/generated/refuel-submit-wallet"
  utxo_file="$tx_dir/utxo.json"
  tx_body="$tx_dir/refuel.body"
  tx_signed="$tx_dir/refuel.signed"
  submit_error_file="$tx_dir/refuel-submit.err"
  target_lovelace=$((TX_REFUEL_UTXO_COUNT * TX_REFUEL_OUTPUT_LOVELACE))
  min_total_lovelace=$((target_lovelace + TX_FEE_BUFFER_LOVELACE))

  mkdir -p "$LOGDIR"
  exec > >(tee -a "$LOGDIR/refuel-submit-wallet.log") 2>&1

  mkdir -p "$tx_dir"
  rm -f "$tx_dir"/* 2>/dev/null || true

  address="$(payment_address "$magic" "$tx_dir/payment.vkey")"
  echo "[refuel-submit-wallet] waiting for upstream cardano-node socket to answer local queries..."
  wait_for_cardano_socket
  wait_for_cardano_query "$magic"

  echo "[refuel-submit-wallet] using payment address: $address"
  query_payment_utxo "$magic" "$socket" "$address" "$utxo_file"

  local clean_count
  clean_count="$(count_clean_refuel_outputs "$utxo_file")"
  if ((clean_count >= TX_REFUEL_UTXO_COUNT)) && ! truthy "$TX_REFUEL_FORCE"; then
    echo "[refuel-submit-wallet] ready: found $clean_count clean outputs of $TX_REFUEL_OUTPUT_LOVELACE lovelace; set TX_REFUEL_FORCE=true to rebuild them anyway"
    clear_submit_claim_state
    echo "[refuel-submit-wallet] cleared submit-tx claim state"
    return 0
  fi
  if truthy "$TX_REFUEL_FORCE"; then
    echo "[refuel-submit-wallet] TX_REFUEL_FORCE=true; rebuilding even though $clean_count clean outputs are already visible"
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
    IFS=$'\t' read -r tx_in lovelace <<< "$record"
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

  if ((input_count == 0)); then
    die "no UTxO found for $address; fund the address before refuelling"
  fi
  if ((total_lovelace < min_total_lovelace)); then
    die "selected $total_lovelace lovelace from $input_count inputs, but at least $min_total_lovelace is needed for ${TX_REFUEL_UTXO_COUNT}x${TX_REFUEL_OUTPUT_LOVELACE} lovelace plus fee buffer"
  fi

  local -a build_args=()
  for tx_in in "${selected_tx_ins[@]}"; do
    build_args+=(--tx-in "$tx_in")
  done
  for ((i = 0; i < TX_REFUEL_UTXO_COUNT; i++)); do
    build_args+=(--tx-out "$address+$TX_REFUEL_OUTPUT_LOVELACE")
  done

  echo "[refuel-submit-wallet] building refuel transaction from $input_count inputs totaling $total_lovelace lovelace into ${TX_REFUEL_UTXO_COUNT} outputs of $TX_REFUEL_OUTPUT_LOVELACE lovelace..."
  "$CARDANO_CLI" conway transaction build \
    --testnet-magic "$magic" \
    --socket-path "$socket" \
    "${build_args[@]}" \
    --change-address "$address" \
    --out-file "$tx_body"

  "$CARDANO_CLI" conway transaction sign \
    --testnet-magic "$magic" \
    --tx-body-file "$tx_body" \
    --signing-key-file "$TX_PAYMENT_SKEY" \
    --out-file "$tx_signed"

  tx_id="$("$CARDANO_CLI" conway transaction txid --tx-file "$tx_signed" --output-text)"
  echo "[refuel-submit-wallet] submitting refuel transaction tx_id=$tx_id"
  if ! "$CARDANO_CLI" conway transaction submit \
    --testnet-magic "$magic" \
    --socket-path "$socket" \
    --tx-file "$tx_signed" 2> "$submit_error_file"; then
    if grep -qi 'All inputs are spent' "$submit_error_file"; then
      cat "$submit_error_file"
      echo "[refuel-submit-wallet] submit input is already spent; continuing as tx_id=$tx_id may already be included"
    else
      cat "$submit_error_file"
      return 1
    fi
  fi

  clear_submit_claim_state
  echo "[refuel-submit-wallet] cleared submit-tx claim state"
  echo "[refuel-submit-wallet] waiting for ${TX_REFUEL_UTXO_COUNT} clean outputs from tx_id=$tx_id..."

  local deadline=$((SECONDS + TX_REFUEL_CONFIRM_TIMEOUT_SECONDS))
  while ((SECONDS < deadline)); do
    query_payment_utxo "$magic" "$socket" "$address" "$utxo_file"
    clean_count="$(count_clean_refuel_outputs "$utxo_file")"
    if ((clean_count >= TX_REFUEL_UTXO_COUNT)); then
      echo "[refuel-submit-wallet] ready: found $clean_count clean outputs after tx_id=$tx_id"
      return 0
    fi
    echo "[refuel-submit-wallet] found $clean_count/${TX_REFUEL_UTXO_COUNT} clean outputs; waiting..."
    sleep 5
  done

  die "timed out waiting for refuel outputs from tx_id=$tx_id; the transaction may still confirm later"
}
