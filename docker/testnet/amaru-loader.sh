#!/usr/bin/env bash
# copy data for amaru nodes

set -vx

copy_databases() {
    target=/state/$1

    cp -fr /data/ledger.testnet\:42.db "$target/ledger.db"
    cp -fr /data/chain.testnet\:42.db "$target/chain.db"
}

nodes=$(ls -d /state/*)
number_of_nodes=$(ls -d /state/* | wc -l)
echo "number_of_nodes: $number_of_nodes"
for node in $nodes; do
  node_ix=$(echo "$node" | awk -F '/' '{print $3}')
  echo "configure node: $node ($node_ix)"
  copy_databases "$node_ix"
done
