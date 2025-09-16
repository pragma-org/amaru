#!/usr/bin/env bash
# copy data for amaru nodes

set -vx

copy_databases() {
    target=/state/$1
    [[ -d  "$target/ledger.db" ]] || mkdir "$target/ledger.db"
    [[ -d  "$target/chain.db" ]] || mkdir "$target/chain.db"

    cp -fr /data/generated/ledger.testnet_42.db/* "$target/ledger.db/"
    cp -fr /data/generated/chain.testnet_42.db/* "$target/chain.db/"
}

# convert ledger states
for i in /data/generated/*; do
    amaru convert-ledger-state --network testnet_42 --snapshot $i --target-dir /data/generated/testnet_42/snapshots
done

# find the last generated nonces file and copy it as 'nonces.json'
last_snapshot=$(ls -1 /data/generated/ |  awk -F '/' '/[0-9]+$/ { print $1 }' | sort -n | tail -1)
cp /data/generated/testnet_42/snapshots/nonces.${last_snapshot}.* /data/generated/testnet_42/nonces.json

# retrieve 4 headers right before snapshot
db-server query --query list-blocks \
          --config /cardano/config/configs/config.json \
          --db /cardano/state | jq -c "[ .[] | select(.slot <= $last_snapshot) ] | .[0:4]" > /data/generated/testnet_42/headers.json

# retrieve actual headers content
mkdir  /data/generated/testnet_42/headers
jq -r '.[] | [ .slot, .hash ] | @csv'  /data/generated/testnet_42/headers.json | tr -d '"' | while IFS=, read -ra hdr ; do
    db-server query --query "get-header ${hdr[0]}.${hdr[1]}" \
              --config /cardano/config/configs/config.json \
              --db /cardano/state >  "/data/generated/testnet_42/headers/header.${hdr[0]}.${hdr[1]}.cbor"
done

# import ledger state
amaru import-ledger-state --network testnet_42 --ledger-dir /data/generated/ledger.testnet_42.db --snapshot-dir /data/generated/testnet_42/snapshots/

# import headers
amaru import-headers --network testnet_42 --chain-dir /data/generated/chain.testnet_42.db --config-dir /data/generated/

# import nonces
amaru import-nonces  --nonces-file /data/generated/testnet_42/nonces.json --network testnet_42 --chain-dir /data/generated/chain.testnet_42.db/

nodes=$(ls -d /state/*)
number_of_nodes=$(ls -d /state/* | wc -l)
echo "number_of_nodes: $number_of_nodes"
for node in $nodes; do
  node_ix=$(echo "$node" | awk -F '/' '{print $3}')
  echo "configure node: $node ($node_ix)"
  copy_databases "$node_ix"
done
