#!/usr/bin/env bash

set -euo pipefail
trap "exit" INT TERM ERR
trap "kill 0" EXIT

NUM_NODES=$1

for i in $(seq 1 $NUM_NODES); do
    NODE_ID=amaru-$i
    amaru bootstrap --ledger-dir "${NODE_ID}/ledger.db" --chain-dir "${NODE_ID}/chain.db" --network preview
    amaru --service-name $NODE_ID --with-open-telemetry daemon --peer-address localhost:3000 --listen-address localhost:$(( 4000 + $i))  --ledger-dir "${NODE_ID}/ledger.db" --chain-dir "${NODE_ID}/chain.db" --network preview > $NODE_ID.log 2>&1 &
    PID=$!
    echo $(date -Iseconds) ": launched $NODE_ID (pid=$PID)"
    sleep 60
done

wait
