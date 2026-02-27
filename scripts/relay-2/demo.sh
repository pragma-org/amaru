#!/usr/bin/env bash
set -euo pipefail

# ---------- config ----------
SESSION="${SESSION:-amaru-relay-2}"

# Derive AMARU_DIR from script location (scripts/relay-2/demo.sh -> amaru root)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
AMARU_DIR="${AMARU_DIR:-$(cd "$SCRIPT_DIR/../.." && pwd)}"

LOGDIR="${LOGDIR:-/tmp/amaru-relay-2}"
RUNDIR="${RUNDIR:-$AMARU_DIR/scripts/relay-2/run}"

# Cardano node configuration
CARDANO_NODE="${CARDANO_NODE:-}"  # path to cardano-node executable
CARDANO_NODE_CONFIG_DIR="${CARDANO_NODE_CONFIG_DIR:-}"  # directory with config.json, topology.json for upstream
CARDANO_NODE_DOWNSTREAM_CONFIG_DIR="${CARDANO_NODE_DOWNSTREAM_CONFIG_DIR:-}"  # directory for downstream cardano-node

# Deterministic ports (can be overridden with env vars)
UPSTREAM_PORT="${UPSTREAM_PORT:-3001}"    # cardano-node upstream listener
LISTEN_PORT="${LISTEN_PORT:-4001}"        # amaru listener (for downstream)
DOWNSTREAM_PORT="${DOWNSTREAM_PORT:-3002}" # cardano-node downstream listener

# ---------- helpers ----------
ensure_dirs() {
  mkdir -p "$LOGDIR" "$RUNDIR"
}

die() { echo "error: $*" >&2; exit 1; }

have() { command -v "$1" >/dev/null 2>&1; }

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
echo "[upstream] starting cardano-node..."
$CARDANO_NODE run --config $CARDANO_NODE_CONFIG_DIR/config.json --topology $CARDANO_NODE_CONFIG_DIR/topology.json --database-path $CARDANO_NODE_CONFIG_DIR/db --socket-path $CARDANO_NODE_CONFIG_DIR/node.socket --port $UPSTREAM_PORT 2>&1 | tee '$LOGDIR/upstream.log'
sleep 999999
EOF
}

cmd_amaru() {
  local delay="${UPSTREAM_INIT_DELAY:-10}"
  cat <<EOF
printf '\033]2;amaru\033\\\\'
echo "[amaru] waiting ${delay}s for upstream cardano-node to initialize..."
sleep $delay
echo "[amaru] starting..."
cd $AMARU_DIR
export AMARU_TRACE=warn,amaru_consensus=debug,amaru::ledger=info
ulimit -n 65536
cargo run --profile dev -- --with-json-traces run --peer-address 127.0.0.1:$UPSTREAM_PORT --listen-address 0.0.0.0:$LISTEN_PORT --chain-dir $RUNDIR/amaru/chain.preprod.db --ledger-dir $RUNDIR/amaru/ledger.preprod.db 2>&1 | tee '$LOGDIR/amaru.log'
sleep 999999
EOF
}

cmd_downstream() {
  local delay="${DOWNSTREAM_INIT_DELAY:-15}"
  cat <<EOF
printf '\033]2;downstream\033\\\\'
echo "[downstream] waiting ${delay}s for amaru to initialize..."
sleep $delay
echo "[downstream] starting cardano-node..."
$CARDANO_NODE run --config $CARDANO_NODE_DOWNSTREAM_CONFIG_DIR/config.json --topology $CARDANO_NODE_DOWNSTREAM_CONFIG_DIR/topology.json --database-path $CARDANO_NODE_DOWNSTREAM_CONFIG_DIR/db --socket-path $CARDANO_NODE_DOWNSTREAM_CONFIG_DIR/node.socket --port $DOWNSTREAM_PORT 2>&1 | tee '$LOGDIR/downstream.log'
sleep 999999
EOF
}

cmd_watch() {
  # Curated view: tweak grep patterns to your log messages (handshake, ChainSync, BlockFetch...)
  cat <<EOF
printf '\033]2;watch\033\\\\'
echo "[watch] tailing logs (Ctrl-c in this pane won't stop the session; use ./demo.sh stop)"
( tail -n +1 -F '$LOGDIR/upstream.log' '$LOGDIR/amaru.log' '$LOGDIR/downstream.log' | sed -E -e 's|^|[log] |' -e 's|\\[upstream\\]|[upstream]|g' ) | grep -E --line-buffered -i 'connected|handshake|chainsync|blockfetch|tip|accepted|peer|error|warn|TODO|\\[log\\]' || true
EOF
}

# ---------- main ----------
start() {
  have tmux || die "tmux not found"
  [[ -n "$CARDANO_NODE" ]] || die "CARDANO_NODE must be set (path to cardano-node executable)"
  [[ -x "$CARDANO_NODE" ]] || die "CARDANO_NODE is not executable: $CARDANO_NODE"
  [[ -n "$CARDANO_NODE_CONFIG_DIR" ]] || die "CARDANO_NODE_CONFIG_DIR must be set (directory with config.json, topology.json, etc.)"
  [[ -d "$CARDANO_NODE_CONFIG_DIR" ]] || die "CARDANO_NODE_CONFIG_DIR does not exist: $CARDANO_NODE_CONFIG_DIR"
  [[ -f "$CARDANO_NODE_CONFIG_DIR/config.json" ]] || die "config.json not found in $CARDANO_NODE_CONFIG_DIR"
  [[ -f "$CARDANO_NODE_CONFIG_DIR/topology.json" ]] || die "topology.json not found in $CARDANO_NODE_CONFIG_DIR"
  [[ -n "$CARDANO_NODE_DOWNSTREAM_CONFIG_DIR" ]] || die "CARDANO_NODE_DOWNSTREAM_CONFIG_DIR must be set (directory for downstream cardano-node)"
  [[ -d "$CARDANO_NODE_DOWNSTREAM_CONFIG_DIR" ]] || die "CARDANO_NODE_DOWNSTREAM_CONFIG_DIR does not exist: $CARDANO_NODE_DOWNSTREAM_CONFIG_DIR"
  [[ -f "$CARDANO_NODE_DOWNSTREAM_CONFIG_DIR/config.json" ]] || die "config.json not found in $CARDANO_NODE_DOWNSTREAM_CONFIG_DIR"
  [[ -f "$CARDANO_NODE_DOWNSTREAM_CONFIG_DIR/topology.json" ]] || die "topology.json not found in $CARDANO_NODE_DOWNSTREAM_CONFIG_DIR"
  [[ -d "$AMARU_DIR" ]] || die "AMARU_DIR does not exist: $AMARU_DIR"

  ensure_dirs
  rm -f "$LOGDIR"/*.log 2>/dev/null || true

  # Copy databases into isolated run directory
  rm -rf "$RUNDIR/amaru"
  mkdir -p "$RUNDIR/amaru"
  cp -r "$AMARU_DIR/chain.preprod.db" "$RUNDIR/amaru/chain.preprod.db"
  cp -r "$AMARU_DIR/ledger.preprod.db" "$RUNDIR/amaru/ledger.preprod.db"

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
  tmux select-pane -t "$SESSION:nodes.1" -T "downstream"
  tmux select-pane -t "$SESSION:nodes.2" -T "amaru"

  # Add watch window
  tmux new-window -t "$SESSION" -n watch -c "$AMARU_DIR"
  tmux select-pane -t "$SESSION:watch.0" -T "watch"

  # Run commands
  pane_run "$SESSION:nodes.0" "$(cmd_upstream)"
  pane_run "$SESSION:nodes.1" "$(cmd_downstream)"
  pane_run "$SESSION:nodes.2" "$(cmd_amaru)"
  pane_run "$SESSION:watch.0" "$(cmd_watch)"

  # Attach
  tmux select-window -t "$SESSION:nodes"
  exec tmux attach -t "$SESSION"
}

restart() {
  local pane="${1:?usage: $0 restart <upstream|amaru|downstream>}"
  [[ -n "$CARDANO_NODE" ]] || die "CARDANO_NODE must be set (path to cardano-node executable)"
  [[ -x "$CARDANO_NODE" ]] || die "CARDANO_NODE is not executable: $CARDANO_NODE"
  [[ -n "$CARDANO_NODE_CONFIG_DIR" ]] || die "CARDANO_NODE_CONFIG_DIR must be set"
  [[ -d "$CARDANO_NODE_CONFIG_DIR" ]] || die "CARDANO_NODE_CONFIG_DIR does not exist: $CARDANO_NODE_CONFIG_DIR"
  [[ -n "$CARDANO_NODE_DOWNSTREAM_CONFIG_DIR" ]] || die "CARDANO_NODE_DOWNSTREAM_CONFIG_DIR must be set"
  [[ -d "$CARDANO_NODE_DOWNSTREAM_CONFIG_DIR" ]] || die "CARDANO_NODE_DOWNSTREAM_CONFIG_DIR does not exist: $CARDANO_NODE_DOWNSTREAM_CONFIG_DIR"
  [[ -d "$AMARU_DIR" ]] || die "AMARU_DIR does not exist: $AMARU_DIR"
  case "$pane" in
    upstream)    pane_run "$SESSION:nodes.0" "$(cmd_upstream)" ;;
    downstream)  pane_run "$SESSION:nodes.1" "$(cmd_downstream)" ;;
    amaru)       pane_run "$SESSION:nodes.2" "$(cmd_amaru)" ;;
    *) die "unknown pane: $pane (choose upstream, amaru, or downstream)" ;;
  esac
}

stop() {
  # Kill processes running in the tmux panes before killing the session
  if tmux has-session -t "$SESSION" 2>/dev/null; then
    # Send SIGTERM to all processes in each pane
    for pane in 0 1 2; do
      tmux send-keys -t "$SESSION:nodes.$pane" C-c 2>/dev/null || true
    done
    tmux send-keys -t "$SESSION:watch.0" C-c 2>/dev/null || true
    sleep 1
  fi

  # Also kill any lingering processes
  pkill -f "cargo run.*--peer-address" 2>/dev/null || true
  pkill -f "target/.*amaru.*run" 2>/dev/null || true
  pkill -f "cardano-node" 2>/dev/null || true

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
  stop) stop ;;
  restart) restart "${2:-}" ;;
  status) status ;;
  *) die "usage: $0 {start|stop|restart <pane>|status}" ;;
esac
