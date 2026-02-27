#!/usr/bin/env bash
set -euo pipefail

# ---------- config ----------
SESSION="${SESSION:-amaru-relay-3}"

# Derive AMARU_DIR from script location (scripts/relay-3/demo.sh -> amaru root)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
AMARU_DIR="${AMARU_DIR:-$(cd "$SCRIPT_DIR/../.." && pwd)}"

LOGDIR="${LOGDIR:-/tmp/amaru-relay-3}"
RUNDIR="${RUNDIR:-$AMARU_DIR/scripts/relay-3/run}"

# Cardano node configuration
CARDANO_NODE="${CARDANO_NODE:-}"  # path to cardano-node executable
CARDANO_NODE_CONFIG_DIR="${CARDANO_NODE_CONFIG_DIR:-}"  # directory with config.json, topology.json for upstream
CARDANO_NODE_DOWNSTREAM_CONFIG_DIR="${CARDANO_NODE_DOWNSTREAM_CONFIG_DIR:-}"  # directory for downstream cardano-node

# Deterministic ports (can be overridden with env vars)
UPSTREAM_PORT="${UPSTREAM_PORT:-3001}"                      # cardano-node upstream listener
AMARU_UPSTREAM_LISTEN_PORT="${AMARU_UPSTREAM_LISTEN_PORT:-4001}"  # amaru-upstream listener
CENTER_LISTEN_PORT="${CENTER_LISTEN_PORT:-4002}"            # amaru-center listener
AMARU_DOWNSTREAM_LISTEN_PORT="${AMARU_DOWNSTREAM_LISTEN_PORT:-4003}"  # amaru-downstream listener
DOWNSTREAM_PORT="${DOWNSTREAM_PORT:-3002}"                  # cardano-node downstream listener

# OpenTelemetry (opt-in): set AMARU_OTEL=true to export spans to an OTLP collector (e.g. otel-ui)
AMARU_OTEL="${AMARU_OTEL:-false}"

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

# Generate otel env exports for a given service name (empty string if AMARU_OTEL is not true)
otel_exports() {
  local service_name="$1"
  if [ "$AMARU_OTEL" = "true" ]; then
    cat <<OTEL
export AMARU_WITH_OPEN_TELEMETRY=true
export OTEL_SERVICE_NAME=$service_name
export OTEL_EXPORTER_OTLP_ENDPOINT="${OTEL_EXPORTER_OTLP_ENDPOINT:-http://localhost:4317}"
OTEL
  fi
}

# ---------- commands ----------
cmd_cardano_upstream() {
  cat <<EOF
printf '\033]2;cardano-upstream\033\\\\'
echo "[cardano-upstream] starting cardano-node..."
$CARDANO_NODE run --config $CARDANO_NODE_CONFIG_DIR/config.json --topology $CARDANO_NODE_CONFIG_DIR/topology.json --database-path $CARDANO_NODE_CONFIG_DIR/db --socket-path $CARDANO_NODE_CONFIG_DIR/node.socket --port $UPSTREAM_PORT 2>&1 | tee '$LOGDIR/cardano-upstream.log'
sleep 999999
EOF
}

cmd_amaru_upstream() {
  local delay="${UPSTREAM_INIT_DELAY:-10}"
  cat <<EOF
printf '\033]2;amaru-upstream\033\\\\'
echo "[amaru-upstream] waiting ${delay}s for cardano-node to initialize..."
sleep $delay
echo "[amaru-upstream] starting..."
cd $AMARU_DIR
$(otel_exports "amaru-upstream")
export AMARU_TRACE=warn,amaru::consensus=info,amaru::ledger=info
ulimit -n 65536
cargo run --profile dev -- --with-json-traces run --peer-address 127.0.0.1:$UPSTREAM_PORT --listen-address 0.0.0.0:$AMARU_UPSTREAM_LISTEN_PORT --chain-dir $RUNDIR/amaru-upstream/chain.preprod.db --ledger-dir $RUNDIR/amaru-upstream/ledger.preprod.db 2>&1 | tee '$LOGDIR/amaru-upstream.log'
sleep 999999
EOF
}

cmd_amaru_center() {
  local delay="${CENTER_INIT_DELAY:-15}"
  cat <<EOF
printf '\033]2;amaru-center\033\\\\'
echo "[amaru-center] waiting ${delay}s for upstream nodes to initialize..."
sleep $delay
echo "[amaru-center] starting..."
cd $AMARU_DIR
$(otel_exports "amaru-center")
export AMARU_TRACE=warn,amaru_consensus=debug,amaru::ledger=info
ulimit -n 65536
cargo run --profile dev -- --with-json-traces run --peer-address 127.0.0.1:$UPSTREAM_PORT --peer-address 127.0.0.1:$AMARU_UPSTREAM_LISTEN_PORT --listen-address 0.0.0.0:$CENTER_LISTEN_PORT --chain-dir $RUNDIR/amaru-center/chain.preprod.db --ledger-dir $RUNDIR/amaru-center/ledger.preprod.db 2>&1 | tee '$LOGDIR/amaru-center.log'
sleep 999999
EOF
}

cmd_amaru_downstream() {
  local delay="${DOWNSTREAM_INIT_DELAY:-20}"
  cat <<EOF
printf '\033]2;amaru-downstream\033\\\\'
echo "[amaru-downstream] waiting ${delay}s for amaru-center to initialize..."
sleep $delay
echo "[amaru-downstream] starting..."
cd $AMARU_DIR
$(otel_exports "amaru-downstream")
export AMARU_TRACE=warn,amaru_consensus=debug,amaru::ledger=info
ulimit -n 65536
cargo run --profile dev -- --with-json-traces run --peer-address 127.0.0.1:$CENTER_LISTEN_PORT --listen-address 0.0.0.0:$AMARU_DOWNSTREAM_LISTEN_PORT --chain-dir $RUNDIR/amaru-downstream/chain.preprod.db --ledger-dir $RUNDIR/amaru-downstream/ledger.preprod.db 2>&1 | tee '$LOGDIR/amaru-downstream.log'
sleep 999999
EOF
}

cmd_cardano_downstream() {
  local delay="${DOWNSTREAM_INIT_DELAY:-20}"
  cat <<EOF
printf '\033]2;cardano-downstream\033\\\\'
echo "[cardano-downstream] waiting ${delay}s for amaru-center to initialize..."
sleep $delay
echo "[cardano-downstream] starting cardano-node..."
$CARDANO_NODE run --config $CARDANO_NODE_DOWNSTREAM_CONFIG_DIR/config.json --topology $CARDANO_NODE_DOWNSTREAM_CONFIG_DIR/topology.json --database-path $CARDANO_NODE_DOWNSTREAM_CONFIG_DIR/db --socket-path $CARDANO_NODE_DOWNSTREAM_CONFIG_DIR/node.socket --port $DOWNSTREAM_PORT 2>&1 | tee '$LOGDIR/cardano-downstream.log'
sleep 999999
EOF
}

cmd_watch() {
  cat <<EOF
printf '\033]2;watch\033\\\\'
echo "[watch] tailing logs (Ctrl-c in this pane won't stop the session; use ./demo.sh stop)"
( tail -n +1 -F '$LOGDIR/cardano-upstream.log' '$LOGDIR/amaru-upstream.log' '$LOGDIR/amaru-center.log' '$LOGDIR/amaru-downstream.log' '$LOGDIR/cardano-downstream.log' | sed -E -e 's|^|[log] |' ) | grep -E --line-buffered -i 'connected|handshake|chainsync|blockfetch|tip|accepted|peer|error|warn|TODO|\\[log\\]' || true
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

  # Copy databases into isolated run directories
  rm -rf "$RUNDIR/amaru-upstream" "$RUNDIR/amaru-center" "$RUNDIR/amaru-downstream"
  mkdir -p "$RUNDIR/amaru-upstream" "$RUNDIR/amaru-center" "$RUNDIR/amaru-downstream"
  cp -r "$AMARU_DIR/chain.preprod.db" "$RUNDIR/amaru-upstream/chain.preprod.db"
  cp -r "$AMARU_DIR/ledger.preprod.db" "$RUNDIR/amaru-upstream/ledger.preprod.db"
  cp -r "$AMARU_DIR/chain.preprod.db" "$RUNDIR/amaru-center/chain.preprod.db"
  cp -r "$AMARU_DIR/ledger.preprod.db" "$RUNDIR/amaru-center/ledger.preprod.db"
  cp -r "$AMARU_DIR/chain.preprod.db" "$RUNDIR/amaru-downstream/chain.preprod.db"
  cp -r "$AMARU_DIR/ledger.preprod.db" "$RUNDIR/amaru-downstream/ledger.preprod.db"

  # reset session
  tmux_kill_session
  tmux_new_session

  # Layout: 3 columns (upstream | center | downstream)
  # ┌──────────────┬──────────────┬──────────────────┐
  # │ cardano-node │              │ cardano-node      │
  # │ upstream     │ amaru-center │ downstream        │
  # ├──────────────┤              ├──────────────────┤
  # │ amaru        │              │ amaru             │
  # │ upstream     │              │ downstream        │
  # └──────────────┴──────────────┴──────────────────┘
  #
  # tmux renumbers panes by position (L→R, T→B) after each split:
  #   split-h on 0:         0=left, 1=right
  #   split-h on 1:         0=left, 1=middle, 2=right
  #   split-v on 0:         0=TL, 1=BL, 2=middle, 3=right
  #   split-v on 3(right):  0=TL, 1=BL, 2=middle, 3=TR, 4=BR
  tmux split-window -t "$SESSION:nodes" -h -c "$AMARU_DIR"       # 0=left, 1=right
  tmux split-window -t "$SESSION:nodes.1" -h -c "$AMARU_DIR"     # 0=left, 1=middle, 2=right
  tmux split-window -t "$SESSION:nodes.0" -v -c "$AMARU_DIR"     # 0=TL, 1=BL, 2=middle, 3=right
  tmux split-window -t "$SESSION:nodes.3" -v -c "$AMARU_DIR"     # 0=TL, 1=BL, 2=middle, 3=TR, 4=BR

  # Name panes
  tmux select-pane -t "$SESSION:nodes.0" -T "cardano-upstream"
  tmux select-pane -t "$SESSION:nodes.1" -T "amaru-upstream"
  tmux select-pane -t "$SESSION:nodes.2" -T "amaru-center"
  tmux select-pane -t "$SESSION:nodes.3" -T "cardano-downstream"
  tmux select-pane -t "$SESSION:nodes.4" -T "amaru-downstream"

  # Add watch window
  tmux new-window -t "$SESSION" -n watch -c "$AMARU_DIR"
  tmux select-pane -t "$SESSION:watch.0" -T "watch"

  # Run commands
  pane_run "$SESSION:nodes.0" "$(cmd_cardano_upstream)"
  pane_run "$SESSION:nodes.1" "$(cmd_amaru_upstream)"
  pane_run "$SESSION:nodes.2" "$(cmd_amaru_center)"
  pane_run "$SESSION:nodes.3" "$(cmd_cardano_downstream)"
  pane_run "$SESSION:nodes.4" "$(cmd_amaru_downstream)"
  pane_run "$SESSION:watch.0" "$(cmd_watch)"

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
  local pane="${1:?usage: $0 restart <cardano-upstream|amaru-upstream|amaru-center|amaru-downstream|cardano-downstream>}"
  [[ -n "$CARDANO_NODE" ]] || die "CARDANO_NODE must be set (path to cardano-node executable)"
  [[ -x "$CARDANO_NODE" ]] || die "CARDANO_NODE is not executable: $CARDANO_NODE"
  [[ -n "$CARDANO_NODE_CONFIG_DIR" ]] || die "CARDANO_NODE_CONFIG_DIR must be set"
  [[ -d "$CARDANO_NODE_CONFIG_DIR" ]] || die "CARDANO_NODE_CONFIG_DIR does not exist: $CARDANO_NODE_CONFIG_DIR"
  [[ -n "$CARDANO_NODE_DOWNSTREAM_CONFIG_DIR" ]] || die "CARDANO_NODE_DOWNSTREAM_CONFIG_DIR must be set"
  [[ -d "$CARDANO_NODE_DOWNSTREAM_CONFIG_DIR" ]] || die "CARDANO_NODE_DOWNSTREAM_CONFIG_DIR does not exist: $CARDANO_NODE_DOWNSTREAM_CONFIG_DIR"
  [[ -d "$AMARU_DIR" ]] || die "AMARU_DIR does not exist: $AMARU_DIR"
  case "$pane" in
    cardano-upstream)    restart_pane "$SESSION:nodes.0" "$(cmd_cardano_upstream)" ;;
    amaru-upstream)      restart_pane "$SESSION:nodes.1" "$(cmd_amaru_upstream)" ;;
    amaru-center)        restart_pane "$SESSION:nodes.2" "$(cmd_amaru_center)" ;;
    cardano-downstream)  restart_pane "$SESSION:nodes.3" "$(cmd_cardano_downstream)" ;;
    amaru-downstream)    restart_pane "$SESSION:nodes.4" "$(cmd_amaru_downstream)" ;;
    *) die "unknown pane: $pane (choose cardano-upstream, amaru-upstream, amaru-center, amaru-downstream, or cardano-downstream)" ;;
  esac
}

stop() {
  # Kill processes running in the tmux panes before killing the session
  if tmux has-session -t "$SESSION" 2>/dev/null; then
    # Send SIGTERM to all processes in each pane
    for pane in 0 1 2 3 4; do
      tmux send-keys -t "$SESSION:nodes.$pane" C-c 2>/dev/null || true
    done
    tmux send-keys -t "$SESSION:watch.0" C-c 2>/dev/null || true
    sleep 1
  fi

  # Also kill any lingering processes (scoped to this demo only)
  pkill -f -- "--chain-dir $RUNDIR/amaru-upstream/chain.preprod.db" 2>/dev/null || true
  pkill -f -- "--chain-dir $RUNDIR/amaru-center/chain.preprod.db" 2>/dev/null || true
  pkill -f -- "--chain-dir $RUNDIR/amaru-downstream/chain.preprod.db" 2>/dev/null || true
  pkill -f -- "--socket-path $CARDANO_NODE_CONFIG_DIR/node.socket" 2>/dev/null || true
  pkill -f -- "--socket-path $CARDANO_NODE_DOWNSTREAM_CONFIG_DIR/node.socket" 2>/dev/null || true


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
