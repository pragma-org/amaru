#!/usr/bin/env bash
# copy data for amaru nodes

set -vx

BASEDIR=${1:-/data/generated}
CONFIG_DIR=${2:-/cardano/config}
CHAIN_DB_DIR=${3:-/cardano/state}

# Implement sponge-like command without the need for binary nor TMPDIR environment variable
write_file() {
    # Create temporary file
    local tmp_file="${1}_$(tr </dev/urandom -dc A-Za-z0-9 | head -c16)"

    # Redirect the output to the temporary file
    cat >"${tmp_file}"

    # Replace the original file
    mv --force "${tmp_file}" "${1}"
}


copy_databases() {
    target=/state/$1
    [[ -d  "$target/ledger.db" ]] || mkdir "$target/ledger.db"
    [[ -d  "$target/chain.db" ]] || mkdir "$target/chain.db"

    cp -fr ${BASEDIR}/ledger.testnet_42.db/* "$target/ledger.db/"
    cp -fr ${BASEDIR}/chain.testnet_42.db/* "$target/chain.db/"
}

# convert ledger states
for i in ${BASEDIR}/*; do
    amaru convert-ledger-state --network testnet_42 --snapshot $i --target-dir ${BASEDIR}/testnet_42/snapshots
done

# find the last generated nonces file and copy it as 'nonces.json'
last_snapshot=$(ls -1 ${BASEDIR}/ |  awk -F '/' '/[0-9]+$/ { print $1 }' | sort -n | tail -1)
cp ${BASEDIR}/testnet_42/snapshots/nonces.${last_snapshot}.* ${BASEDIR}/testnet_42/nonces.json

# retrieve 4 headers right before snapshot
db-server query --query list-blocks \
          --config ${CONFIG_DIR}/configs/config.json \
          --db ${CHAIN_DB_DIR} | jq -c "[ .[] | select(.slot <= $last_snapshot) ] | .[0:4]" > ${BASEDIR}/testnet_42/headers.json

# retrieve actual headers content
mkdir  ${BASEDIR}/testnet_42/headers
jq -r '.[] | [ .slot, .hash ] | @csv'  ${BASEDIR}/testnet_42/headers.json | tr -d '"' | while IFS=, read -ra hdr ; do
    db-server query --query "get-header ${hdr[0]}.${hdr[1]}" \
              --config ${CONFIG_DIR}/configs/config.json \
              --db ${CHAIN_DB_DIR} >  "${BASEDIR}/testnet_42/headers/header.${hdr[0]}.${hdr[1]}.cbor"
done

# update the nonces' tail with the grandparent of the last header of the epoch
second_to_last_hash=$(ls -1 ${BASEDIR}/testnet_42/headers | cut -d '.' -f 2,3 | sort -n | cut -d '.' -f 2 | tail  -3 | head -1)
jq ".tail = \"${second_to_last_hash}\"" ${BASEDIR}/testnet_42/nonces.json  | write_file  ${BASEDIR}/testnet_42/nonces.json

# import ledger state
amaru import-ledger-state --network testnet_42 --ledger-dir ${BASEDIR}/ledger.testnet_42.db --snapshot-dir ${BASEDIR}/testnet_42/snapshots/

# import headers
amaru import-headers --network testnet_42 --chain-dir ${BASEDIR}/chain.testnet_42.db --config-dir ${BASEDIR}/

# import nonces
amaru import-nonces  --nonces-file ${BASEDIR}/testnet_42/nonces.json --network testnet_42 --chain-dir ${BASEDIR}/chain.testnet_42.db/

nodes=$(ls -d /state/*)
number_of_nodes=$(ls -d /state/* | wc -l)
echo "number_of_nodes: $number_of_nodes"
for node in $nodes; do
  node_ix=$(echo "$node" | awk -F '/' '{print $3}')
  echo "configure node: $node ($node_ix)"
  copy_databases "$node_ix"
done
