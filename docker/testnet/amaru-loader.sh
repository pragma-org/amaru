#!/usr/bin/env bash
# copy data for amaru nodes

copy_databases() {
    target=/state/$1

    cp -fr ledger.testnet\:42.db "$target"
    cp -fr chain.testnet\:42.db "$target"
}

pools=$(ls -d /state/*)
number_of_pools=$(ls -d /state/* | wc -l)
echo "number_of_nodes: $number_of_nodes"
for node in $nodes; do
  node_ix=$(echo "$node" | awk -F '/' '{print $3}')
  echo "configure node: $node ($node_ix)"
  copy_databases "$node_ix"
done
