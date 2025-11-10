#!/usr/bin/env bash
# This script bootstraps and launches multiple Amaru nodes connecting to
# a specified peer address. It takes two arguments: the number of nodes to
# launch and the peer address to connect to.
# It assumes that Amaru is installed and available in the system's PATH.
#
# NOTE: This script is intended mostly for testing purposes, eg. ensuring
# that multiple nodes can connect to a single peer without issues and that
# resources required stay within requirements.


set -euo pipefail
trap "exit" INT TERM ERR
trap "kill 0" EXIT

NUM_NODES=$1
PEER_ADDRESS=$2

for i in $(seq 1 "$NUM_NODES"); do
    NODE_ID=amaru-$i
    echo "$(date -Iseconds) : bootstrapping $NODE_ID"
    amaru bootstrap --ledger-dir "${NODE_ID}/ledger.db" --chain-dir "${NODE_ID}/chain.db" --network preview > "$NODE_ID.log" 2>&1
    amaru daemon --peer-address "${PEER_ADDRESS}" \
          --listen-address localhost:$(( 4000 + i)) \
          --ledger-dir "${NODE_ID}/ledger.db" \
          --chain-dir "${NODE_ID}/chain.db" \
          --network preview >> "$NODE_ID.log" 2>&1 &
    PID=$!
    echo "$(date -Iseconds) : launched $NODE_ID (pid=$PID)"
    sleep 60
done

wait
