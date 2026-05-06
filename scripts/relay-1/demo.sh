#!/usr/bin/env bash
set -euo pipefail

# ---------- config ----------
SESSION="${SESSION:-amaru-demo}"

# Derive AMARU_DIR from script location (scripts/relay-1/demo.sh -> amaru root)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
AMARU_DIR="${AMARU_DIR:-$(cd "$SCRIPT_DIR/../.." && pwd)}"

LOGDIR="${LOGDIR:-/tmp/amaru-relay-1}"
RUNDIR="${RUNDIR:-$AMARU_DIR/scripts/relay-1/run}"

# Cardano node configuration
CARDANO_NODE="${CARDANO_NODE:-}"  # path to cardano-node executable
CARDANO_NODE_CONFIG_DIR="${CARDANO_NODE_CONFIG_DIR:-}"  # directory with config.json, topology.json, etc.
CARDANO_CLI="${CARDANO_CLI:-$(command -v cardano-cli || true)}"  # path to cardano-cli executable
CARDANO_TESTNET_MAGIC="${CARDANO_TESTNET_MAGIC:-}"  # inferred from config.json when empty

# Deterministic ports (can be overridden with env vars)
UPSTREAM_PORT="${UPSTREAM_PORT:-3001}" # cardano-node listener
LISTEN_PORT="${LISTEN_PORT:-4001}"  # amaru listener (for downstream)
DOWNSTREAM_LISTEN_PORT="${DOWNSTREAM_LISTEN_PORT:-4002}"  # amaru downstream listener
DOWNSTREAM_SUBMIT_API_ADDRESS="${DOWNSTREAM_SUBMIT_API_ADDRESS:-127.0.0.1:8091}"  # downstream submit API
TX_CBOR_FILE="${TX_CBOR_FILE:-}"  # optional transaction CBOR file to submit after startup
TX_CBOR_FILES="${TX_CBOR_FILES:-$TX_CBOR_FILE}"  # optional whitespace-separated transaction CBOR files
TX_PAYMENT_SKEY="${TX_PAYMENT_SKEY:-}"  # optional funded payment signing key for runtime tx generation
TX_PAYMENT_VKEY="${TX_PAYMENT_VKEY:-}"  # optional payment verification key; derived from TX_PAYMENT_SKEY when empty
TX_PAYMENT_ADDRESS="${TX_PAYMENT_ADDRESS:-}"  # optional payment address; derived from TX_PAYMENT_VKEY when empty
TX_GENERATED_COUNT="${TX_GENERATED_COUNT:-3}"  # number of runtime transactions to build and submit
TX_OUTPUT_LOVELACE="${TX_OUTPUT_LOVELACE:-1000000}"  # self-transfer output amount per generated transaction
TX_MIN_INPUT_LOVELACE="${TX_MIN_INPUT_LOVELACE:-3000000}"  # preferred minimum input amount for multi-tx generation
TX_SYNC_SLOT_TOLERANCE="${TX_SYNC_SLOT_TOLERANCE:-100}"  # max slot gap allowed between downstream Amaru and upstream cardano-node before tx generation
TX_SYNC_TIMEOUT_SECONDS="${TX_SYNC_TIMEOUT_SECONDS:-14400}"  # how long to wait for downstream Amaru to catch up (default 4h to allow ledger replay from bootstrap)
TX_SUBMIT_RETRY_LIMIT="${TX_SUBMIT_RETRY_LIMIT:-12}"  # number of retries if downstream rejects with missing-inputs (UTxO not yet in downstream ledger)
TX_SUBMIT_RETRY_DELAY="${TX_SUBMIT_RETRY_DELAY:-30}"  # seconds between retries

# ---------- helpers ----------
ensure_dirs() {
  mkdir -p "$LOGDIR" "$RUNDIR"
}

die() { echo "error: $*" >&2; exit 1; }

have() { command -v "$1" >/dev/null 2>&1; }

require_cardano_node() {
  [[ -n "$CARDANO_NODE" ]] || die "CARDANO_NODE must be set (path to cardano-node executable)"
  [[ -f "$CARDANO_NODE" ]] || die "CARDANO_NODE must point to the executable file, not a directory: $CARDANO_NODE"
  [[ -x "$CARDANO_NODE" ]] || die "CARDANO_NODE is not executable: $CARDANO_NODE"
}

require_tx_file() {
  [[ -n "$1" ]] || die "transaction CBOR file path must be provided"
  [[ -f "$1" ]] || die "transaction CBOR file does not exist: $1"
}

require_cardano_cli() {
  [[ -n "$CARDANO_CLI" ]] || die "CARDANO_CLI must be set or available on PATH"
  [[ -x "$CARDANO_CLI" ]] || die "CARDANO_CLI is not executable: $CARDANO_CLI"
}

network_magic() {
  if [[ -n "$CARDANO_TESTNET_MAGIC" ]]; then
    echo "$CARDANO_TESTNET_MAGIC"
  elif have jq && [[ -f "$CARDANO_NODE_CONFIG_DIR/config.json" ]]; then
    jq -r '.NetworkMagic // 1' "$CARDANO_NODE_CONFIG_DIR/config.json"
  else
    echo 1
  fi
}

downstream_adopted_slot() {
  local log="$LOGDIR/amaru-downstream.log"
  [[ -f "$log" ]] || { echo ""; return; }
  LC_ALL=C grep -F '"adopted tip"' "$log" 2>/dev/null \
    | tail -n 1 \
    | sed -nE 's/.*"tip\.slot":"([0-9]+)".*/\1/p'
}

upstream_tip_slot() {
  local magic="$1" socket="$2"
  "$CARDANO_CLI" conway query tip --testnet-magic "$magic" --socket-path "$socket" 2>/dev/null \
    | jq -r '.slot // empty'
}

wait_for_downstream_sync() {
  local magic="$1" socket="$2"
  local tolerance="$TX_SYNC_SLOT_TOLERANCE"
  local timeout="$TX_SYNC_TIMEOUT_SECONDS"
  local start now upstream downstream gap elapsed prev_down prev_t rate eta
  start="$(date +%s)"
  prev_down=""
  prev_t=""
  echo "[submit-tx] waiting for downstream Amaru to catch up to upstream tip (tolerance: ${tolerance} slots, timeout: ${timeout}s)..."
  echo "[submit-tx] note: Amaru replays the ledger from its bootstrap snapshot, which can take ~1-2 hours to reach network tip"
  while true; do
    now="$(date +%s)"
    elapsed=$(( now - start ))
    upstream="$(upstream_tip_slot "$magic" "$socket")"
    downstream="$(downstream_adopted_slot)"
    if [[ -n "$upstream" && -n "$downstream" ]]; then
      gap=$(( upstream - downstream ))
      eta=""
      if [[ -n "$prev_down" && -n "$prev_t" && "$now" -gt "$prev_t" ]]; then
        local d_slots d_secs
        d_slots=$(( downstream - prev_down ))
        d_secs=$(( now - prev_t ))
        if (( d_slots > d_secs )); then
          rate=$(( (d_slots - d_secs) ))
          if (( rate > 0 )); then
            eta=" eta=$(( gap * d_secs / rate ))s"
          fi
        fi
      fi
      printf '[submit-tx] elapsed=%ss upstream=%s downstream=%s gap=%s%s\n' "$elapsed" "$upstream" "$downstream" "$gap" "$eta"
      if (( gap <= tolerance )); then
        echo "[submit-tx] downstream caught up; proceeding"
        return 0
      fi
      prev_down="$downstream"
      prev_t="$now"
    else
      echo "[submit-tx] elapsed=${elapsed}s upstream=${upstream:-?} downstream=${downstream:-?} (still initializing)"
    fi
    if (( elapsed > timeout )); then
      die "downstream Amaru did not catch up within ${timeout}s (last gap: ${gap:-unknown})"
    fi
    sleep 15
  done
}

submit_tx_with_retry() {
  local tx_file="$1" attempt="${2:-1}"
  local response status body retries="$TX_SUBMIT_RETRY_LIMIT" delay="$TX_SUBMIT_RETRY_DELAY"
  local response_file="$RUNDIR/generated/last-response.txt"
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

payment_address() {
  local magic="$1"
  local vkey="$TX_PAYMENT_VKEY"

  if [[ -n "$TX_PAYMENT_ADDRESS" ]]; then
    echo "$TX_PAYMENT_ADDRESS"
    return
  fi

  if [[ -z "$vkey" ]]; then
    vkey="$RUNDIR/generated/payment.vkey"
    "$CARDANO_CLI" conway key verification-key --signing-key-file "$TX_PAYMENT_SKEY" --verification-key-file "$vkey" >/dev/null
  fi

  "$CARDANO_CLI" conway address build --payment-verification-key-file "$vkey" --testnet-magic "$magic"
}

tmux_new_session() {
  tmux new-session -d -s "$SESSION" -c "$AMARU_DIR" -n nodes
  tmux set-option -t "$SESSION" remain-on-exit on
  # Disable automatic renaming so pane titles stay fixed
  tmux set-option -t "$SESSION" allow-rename off
  tmux set-option -t "$SESSION" automatic-rename off
  # Show pane titles in the border with colors
  tmux set-option -t "$SESSION" pane-border-status top
  tmux set-option -t "$SESSION" pane-border-format " #[fg=white,bg=blue,bold] #{pane_title} #[default] "
  tmux set-option -t "$SESSION" pane-border-style "fg=brightblack"
  tmux set-option -t "$SESSION" pane-active-border-style "fg=blue,bold"
  # Enable mouse support and clipboard integration
  tmux set-option -t "$SESSION" mouse on
  tmux set-option -t "$SESSION" set-clipboard on
}

tmux_kill_session() {
  tmux kill-session -t "$SESSION" 2>/dev/null || true
}

# Run a command in a pane, with nice bash defaults
# Usage: pane_run <target-pane> <command>
pane_run() {
  local target="$1"; shift
  local cmd="$*"
  # Join multi-line commands into a single line to avoid multiple prompts
  local oneline
  oneline=$(printf '%s' "$cmd" | tr '\n' ';' | sed 's/;;*/;/g; s/^;//; s/;$//')
  tmux send-keys -t "$target" "set -eo pipefail; cd '$AMARU_DIR'; $oneline" C-m
}

# ---------- commands you must fill ----------
cmd_upstream() {
  cat <<EOF
printf '\033]2;upstream\033\\\\'
echo "[upstream] starting..."
$CARDANO_NODE run --config $CARDANO_NODE_CONFIG_DIR/config.json --topology $CARDANO_NODE_CONFIG_DIR/topology.json --database-path $CARDANO_NODE_CONFIG_DIR/db --socket-path $CARDANO_NODE_CONFIG_DIR/node.socket --port $UPSTREAM_PORT 2>&1 | tee '$LOGDIR/upstream.log'
sleep 999999
EOF
}

cmd_amaru() {
  local delay="${UPSTREAM_INIT_DELAY:-10}"
  cat <<EOF
printf '\033]2;amaru\033\\\\'
echo "[amaru] waiting ${delay}s for cardano-node to initialize..."
sleep $delay
echo "[amaru] starting..."
cd $AMARU_DIR
export AMARU_TRACE=warn,amaru_consensus=debug,amaru::ledger=info
export AMARU_TRACE=\$AMARU_TRACE,amaru_protocols::tx_submission=debug,amaru_consensus::stages::mempool=debug,amaru::submit_api=info
ulimit -n 65536
cargo run --profile dev -- --with-json-traces run --peer-address 127.0.0.1:$UPSTREAM_PORT --listen-address 0.0.0.0:$LISTEN_PORT --chain-dir $RUNDIR/amaru/chain.preprod.db --ledger-dir $RUNDIR/amaru/ledger.preprod.db 2>&1 | tee '$LOGDIR/amaru.log'
sleep 999999
EOF
}

cmd_amaru_downstream() {
  local delay="${DOWNSTREAM_INIT_DELAY:-15}"
  cat <<EOF
printf '\033]2;amaru-downstream\033\\\\'
echo "[amaru-downstream] waiting ${delay}s for amaru to initialize..."
sleep $delay
echo "[amaru-downstream] starting..."
cd $AMARU_DIR
export AMARU_TRACE=warn,amaru_consensus=debug,amaru::ledger=info
export AMARU_TRACE=\$AMARU_TRACE,amaru_protocols::tx_submission=debug,amaru_consensus::stages::mempool=debug,amaru::submit_api=info
ulimit -n 65536
cargo run --profile dev -- --with-json-traces run --peer-address 127.0.0.1:$LISTEN_PORT --listen-address 0.0.0.0:$DOWNSTREAM_LISTEN_PORT --submit-api-address $DOWNSTREAM_SUBMIT_API_ADDRESS --chain-dir $RUNDIR/amaru-downstream/chain.preprod.db --ledger-dir $RUNDIR/amaru-downstream/ledger.preprod.db 2>&1 | tee '$LOGDIR/amaru-downstream.log'
sleep 999999
EOF
}

cmd_submit_tx() {
  local delay="${TX_SUBMIT_DELAY:-45}"
  local tx_files="$1"
  cat <<EOF
printf '\033]2;submit-tx\033\\\\'
echo "[submit-tx] waiting ${delay}s for downstream submit API at $DOWNSTREAM_SUBMIT_API_ADDRESS..."
sleep $delay
for tx_file in $tx_files; do
  echo "[submit-tx] submitting \$tx_file to downstream Amaru..."
  curl -i -sS -X POST -H 'Content-Type: application/cbor' --data-binary "@\$tx_file" 'http://$DOWNSTREAM_SUBMIT_API_ADDRESS/api/submit/tx' 2>&1 | tee -a '$LOGDIR/submit-tx.log'
  echo | tee -a '$LOGDIR/submit-tx.log'
done
echo "[submit-tx] done; watch amaru.log and amaru-downstream.log for tx-submission RequestTx*/ReplyTx* and mempool messages"
sleep 999999
EOF
}

cmd_generate_and_submit_tx() {
  local delay="${TX_SUBMIT_DELAY:-45}"
  cat <<EOF
printf '\033]2;submit-tx\033\\\\'
echo "[submit-tx] waiting ${delay}s for upstream socket and downstream submit API..."
sleep $delay
CARDANO_CLI='$CARDANO_CLI' CARDANO_NODE_CONFIG_DIR='$CARDANO_NODE_CONFIG_DIR' CARDANO_TESTNET_MAGIC='$CARDANO_TESTNET_MAGIC' DOWNSTREAM_SUBMIT_API_ADDRESS='$DOWNSTREAM_SUBMIT_API_ADDRESS' LOGDIR='$LOGDIR' RUNDIR='$RUNDIR' TX_PAYMENT_SKEY='$TX_PAYMENT_SKEY' TX_PAYMENT_VKEY='$TX_PAYMENT_VKEY' TX_PAYMENT_ADDRESS='$TX_PAYMENT_ADDRESS' TX_GENERATED_COUNT='$TX_GENERATED_COUNT' TX_OUTPUT_LOVELACE='$TX_OUTPUT_LOVELACE' '$SCRIPT_DIR/demo.sh' generate-submit
echo "[submit-tx] done; watch amaru.log and amaru-downstream.log for tx-submission RequestTx*/ReplyTx* and mempool messages"
sleep 999999
EOF
}

cmd_watch() {
  # Curated view: tweak grep patterns to your log messages (handshake, ChainSync, BlockFetch…)
  cat <<EOF
printf '\033]2;watch\033\\\\'
echo "[watch] tailing logs (Ctrl-c in this pane won't stop the session; use ./demo.sh stop)"
( tail -n +1 -F '$LOGDIR/upstream.log' '$LOGDIR/amaru.log' '$LOGDIR/amaru-downstream.log' '$LOGDIR/submit-tx.log' 2>/dev/null | sed -E -e 's|^|[log] |' -e 's|\\[upstream\\]|[upstream]|g' ) | grep -E --line-buffered -i 'connected|handshake|chainsync|blockfetch|tx.?submission|requesttx|replytx|mempool|transaction|submit-api|tip|accepted|rejected|peer|error|warn|TODO|\\[log\\]' || true
EOF
}

# ---------- main ----------
start() {
  have tmux || die "tmux not found"
  require_cardano_node
  [[ -n "$CARDANO_NODE_CONFIG_DIR" ]] || die "CARDANO_NODE_CONFIG_DIR must be set (directory with config.json, topology.json, etc.)"
  [[ -d "$CARDANO_NODE_CONFIG_DIR" ]] || die "CARDANO_NODE_CONFIG_DIR does not exist: $CARDANO_NODE_CONFIG_DIR"
  [[ -f "$CARDANO_NODE_CONFIG_DIR/config.json" ]] || die "config.json not found in $CARDANO_NODE_CONFIG_DIR"
  [[ -f "$CARDANO_NODE_CONFIG_DIR/topology.json" ]] || die "topology.json not found in $CARDANO_NODE_CONFIG_DIR"
  [[ -d "$AMARU_DIR" ]] || die "AMARU_DIR does not exist: $AMARU_DIR"

  ensure_dirs
  rm -f "$LOGDIR"/*.log 2>/dev/null || true

  # Copy databases into isolated run directories
  rm -rf "$RUNDIR/amaru" "$RUNDIR/amaru-downstream"
  mkdir -p "$RUNDIR/amaru" "$RUNDIR/amaru-downstream"
  cp -r "$AMARU_DIR/chain.preprod.db" "$RUNDIR/amaru/chain.preprod.db"
  cp -r "$AMARU_DIR/ledger.preprod.db" "$RUNDIR/amaru/ledger.preprod.db"
  cp -r "$AMARU_DIR/chain.preprod.db" "$RUNDIR/amaru-downstream/chain.preprod.db"
  cp -r "$AMARU_DIR/ledger.preprod.db" "$RUNDIR/amaru-downstream/ledger.preprod.db"

  # reset session
  tmux_kill_session
  tmux_new_session

  # Layout:
  # nodes window: left split top/bottom = upstream/downstream, right = amaru
  # Pane numbers after splits: 0=top-left, 1=bottom-left, 2=right
  tmux split-window -t "$SESSION:nodes" -h -c "$AMARU_DIR"     # create right pane
  tmux split-window -t "$SESSION:nodes.0" -v -c "$AMARU_DIR"   # split left into upstream/downstream

  tmux select-pane -t "$SESSION:nodes.0" \; select-pane -t "$SESSION:nodes.1"

  # Name panes
  tmux select-pane -t "$SESSION:nodes.0" -T "upstream"
  tmux select-pane -t "$SESSION:nodes.1" -T "amaru-downstream"
  tmux select-pane -t "$SESSION:nodes.2" -T "amaru"

  # Add watch window
  tmux new-window -t "$SESSION" -n watch -c "$AMARU_DIR"
  tmux select-pane -t "$SESSION:watch.0" -T "watch"

  if [[ -n "$TX_CBOR_FILES" ]]; then
    for tx_file in $TX_CBOR_FILES; do
      require_tx_file "$tx_file"
    done
  elif [[ -n "$TX_PAYMENT_SKEY" ]]; then
    require_cardano_cli
    have jq || die "jq is required to generate transactions"
    have curl || die "curl not found"
    [[ -f "$TX_PAYMENT_SKEY" ]] || die "TX_PAYMENT_SKEY does not exist: $TX_PAYMENT_SKEY"
  fi

  if [[ -n "$TX_CBOR_FILES" || -n "$TX_PAYMENT_SKEY" ]]; then
    tmux new-window -t "$SESSION" -n submit -c "$AMARU_DIR"
    tmux select-pane -t "$SESSION:submit.0" -T "submit-tx"
  fi

  # Run commands
  pane_run "$SESSION:nodes.0" "$(cmd_upstream)"
  pane_run "$SESSION:nodes.1" "$(cmd_amaru_downstream)"
  pane_run "$SESSION:nodes.2" "$(cmd_amaru)"
  pane_run "$SESSION:watch.0" "$(cmd_watch)"
  if [[ -n "$TX_CBOR_FILES" ]]; then
    pane_run "$SESSION:submit.0" "$(cmd_submit_tx "$TX_CBOR_FILES")"
  elif [[ -n "$TX_PAYMENT_SKEY" ]]; then
    pane_run "$SESSION:submit.0" "$(cmd_generate_and_submit_tx)"
  fi

  # Attach
  tmux select-window -t "$SESSION:nodes"
  exec tmux attach -t "$SESSION"
}

restart_pane() {
  local target="$1"; shift
  local cmd="$*"
  tmux send-keys -t "$target" C-c 2>/dev/null || true
  sleep 1
  pane_run "$target" "$cmd"
}

restart() {
  local pane="${1:?usage: $0 restart <upstream|amaru|amaru-downstream>}"
  require_cardano_node
  [[ -n "$CARDANO_NODE_CONFIG_DIR" ]] || die "CARDANO_NODE_CONFIG_DIR must be set"
  [[ -d "$CARDANO_NODE_CONFIG_DIR" ]] || die "CARDANO_NODE_CONFIG_DIR does not exist: $CARDANO_NODE_CONFIG_DIR"
  [[ -d "$AMARU_DIR" ]] || die "AMARU_DIR does not exist: $AMARU_DIR"
  case "$pane" in
    upstream)          restart_pane "$SESSION:nodes.0" "$(cmd_upstream)" ;;
    amaru-downstream)  restart_pane "$SESSION:nodes.1" "$(cmd_amaru_downstream)" ;;
    amaru)             restart_pane "$SESSION:nodes.2" "$(cmd_amaru)" ;;
    *) die "unknown pane: $pane (choose upstream, amaru, or amaru-downstream)" ;;
  esac
}

submit() {
  local tx_files=("$@")
  if [[ ${#tx_files[@]} -eq 0 && -n "$TX_CBOR_FILES" ]]; then
    read -r -a tx_files <<< "$TX_CBOR_FILES"
  fi
  [[ ${#tx_files[@]} -gt 0 ]] || die "transaction CBOR file path must be provided"
  have curl || die "curl not found"
  mkdir -p "$LOGDIR"
  : > "$LOGDIR/submit-tx.log"
  for tx_file in "${tx_files[@]}"; do
    require_tx_file "$tx_file"
    echo "submitting $tx_file to downstream Amaru at $DOWNSTREAM_SUBMIT_API_ADDRESS"
    curl -i -sS -X POST -H 'Content-Type: application/cbor' --data-binary "@$tx_file" "http://$DOWNSTREAM_SUBMIT_API_ADDRESS/api/submit/tx" \
      2>&1 | tee -a "$LOGDIR/submit-tx.log"
    echo | tee -a "$LOGDIR/submit-tx.log"
  done
}

generate_submit() {
  require_cardano_cli
  [[ -n "$CARDANO_NODE_CONFIG_DIR" ]] || die "CARDANO_NODE_CONFIG_DIR must be set"
  [[ -n "$TX_PAYMENT_SKEY" ]] || die "TX_PAYMENT_SKEY must be set to a funded preprod payment signing key"
  [[ -f "$TX_PAYMENT_SKEY" ]] || die "TX_PAYMENT_SKEY does not exist: $TX_PAYMENT_SKEY"
  have jq || die "jq is required to query and select UTxOs"
  have curl || die "curl not found"

  local socket="$CARDANO_NODE_CONFIG_DIR/node.socket"
  local magic address tx_dir utxo_file min_lovelace
  magic="$(network_magic)"
  tx_dir="$RUNDIR/generated"
  utxo_file="$tx_dir/utxo.json"
  min_lovelace="$TX_MIN_INPUT_LOVELACE"

  mkdir -p "$tx_dir" "$LOGDIR"
  : > "$LOGDIR/submit-tx.log"
  exec > >(tee -a "$LOGDIR/submit-tx.log") 2>&1

  for _ in {1..60}; do
    [[ -S "$socket" ]] && break
    sleep 1
  done
  [[ -S "$socket" ]] || die "cardano-node socket not found: $socket"

  wait_for_downstream_sync "$magic" "$socket"

  address="$(payment_address "$magic")"
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

  local -a tx_records=()
  while IFS= read -r record; do
    tx_records+=("$record")
  done < <(
    jq -r '
      to_entries[]
      | [.key, (.value.value.lovelace // 0)]
      | @tsv
    ' "$utxo_file" | sort -k2,2nr
  )

  [[ ${#tx_records[@]} -gt 0 ]] || die "no UTxO found for $address"

  local -a selected_tx_ins=()
  local record tx_in lovelace
  for record in "${tx_records[@]}"; do
    IFS=$'\t' read -r tx_in lovelace <<< "$record"
    if [[ "$lovelace" -ge "$min_lovelace" ]]; then
      selected_tx_ins+=("$tx_in")
    fi
  done

  if [[ ${#selected_tx_ins[@]} -eq 0 ]]; then
    IFS=$'\t' read -r tx_in _ <<< "${tx_records[0]}"
    selected_tx_ins=("$tx_in")
    echo "[submit-tx] no UTxO reached the preferred $min_lovelace lovelace threshold; falling back to one tx from the largest available input"
  else
    selected_tx_ins=("${selected_tx_ins[@]:0:TX_GENERATED_COUNT}")
  fi

  local index=1
  local tx_files=()
  for tx_in in "${selected_tx_ins[@]}"; do
    local tx_body="$tx_dir/tx-$index.body"
    local tx_json="$tx_dir/tx-$index.json"
    local tx_cbor="$tx_dir/tx-$index.cbor"

    echo "[submit-tx] building transaction $index from $tx_in..."
    "$CARDANO_CLI" conway transaction build \
      --testnet-magic "$magic" \
      --socket-path "$socket" \
      --tx-in "$tx_in" \
      --tx-out "$address+$TX_OUTPUT_LOVELACE" \
      --change-address "$address" \
      --out-file "$tx_body"

    "$CARDANO_CLI" conway transaction sign \
      --testnet-magic "$magic" \
      --tx-body-file "$tx_body" \
      --signing-key-file "$TX_PAYMENT_SKEY" \
      --out-canonical-cbor \
      --out-file "$tx_json"

    jq -r '.cborHex' "$tx_json" | xxd -r -p > "$tx_cbor"

    tx_files+=("$tx_cbor")
    index=$((index + 1))
  done

  for tx_file in "${tx_files[@]}"; do
    submit_tx_with_retry "$tx_file" || echo "[submit-tx] tx $tx_file was not accepted; continuing with remaining transactions"
  done
}

stop() {
  # Kill processes running in the tmux panes before killing the session
  if tmux has-session -t "$SESSION" 2>/dev/null; then
    # Send SIGTERM to all processes in each pane
    for pane in 0 1 2; do
      tmux send-keys -t "$SESSION:nodes.$pane" C-c 2>/dev/null || true
    done
    tmux send-keys -t "$SESSION:watch.0" C-c 2>/dev/null || true
    tmux send-keys -t "$SESSION:submit.0" C-c 2>/dev/null || true
    sleep 1
  fi

  # Also kill any lingering processes (scoped to this demo only)
  pkill -f -- "--chain-dir $RUNDIR/amaru/chain.preprod.db" 2>/dev/null || true
  pkill -f -- "--chain-dir $RUNDIR/amaru-downstream/chain.preprod.db" 2>/dev/null || true
  pkill -f -- "--socket-path $CARDANO_NODE_CONFIG_DIR/node.socket" 2>/dev/null || true

  tmux_kill_session
  echo "stopped tmux session: $SESSION"
}

status() {
  if tmux has-session -t "$SESSION" 2>/dev/null; then
    echo "running: $SESSION"
    tmux list-windows -t "$SESSION"
  else
    echo "not running: $SESSION"
  fi
}

case "${1:-start}" in
  start) start ;;
  submit) shift; submit "$@" ;;
  generate-submit) generate_submit ;;
  stop) stop ;;
  restart) restart "${2:-}" ;;
  status) status ;;
  *) die "usage: $0 {start|submit <tx.cbor> [tx.cbor ...]|generate-submit|stop|restart <pane>|status}" ;;
esac
