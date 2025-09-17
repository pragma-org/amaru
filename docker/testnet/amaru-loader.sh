#!/usr/bin/env bash
# copy data for amaru nodes

set -vx

BASEDIR=${BASEDIR:-/data/generated}
CONFIG_DIR=${CONFIG_DIR:-/cardano/config}
CHAIN_DB_DIR=${CHAIN_DB_DIR:-/cardano/state}

# TODO: derive from cardano-node's configuration files?
NETWORK_NAME=${NETWORK_NAME:-testnet_42}

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

    cp -fr ${BASEDIR}/ledger.${NETWORK_NAME}.db/* "$target/ledger.db/"
    cp -fr ${BASEDIR}/chain.${NETWORK_NAME}.db/* "$target/chain.db/"
}

# convert ledger states
for i in ${BASEDIR}/*; do
    amaru convert-ledger-state --network ${NETWORK_NAME} --snapshot $i --target-dir ${BASEDIR}/${NETWORK_NAME}/snapshots
done

# find the last generated nonces file and copy it as 'nonces.json'
last_snapshot=$(ls -1 ${BASEDIR}/ |  awk -F '/' '/[0-9]+$/ { print $1 }' | sort -n | tail -1)
cp ${BASEDIR}/${NETWORK_NAME}/snapshots/nonces.${last_snapshot}.* ${BASEDIR}/${NETWORK_NAME}/nonces.json

# retrieve second to last snapshot's slot to query headers later
second_to_last_snapshot=$(ls -1 ${BASEDIR}/ |  awk -F '/' '/[0-9]+$/ { print $1 }' | sort -n | tail -2 | head -1)

# update the nonces' `tail` with the last header hash of the previous epoch
second_to_last_hash=$(ls -1 ${BASEDIR}/${NETWORK_NAME}/snapshots |  awk -F '/' '/.*.cbor$/ { print $1 }' | sort -n | cut -d '.' -f 2 | tail -2 | head -1)
jq ".tail = \"${second_to_last_hash}\"" ${BASEDIR}/${NETWORK_NAME}/nonces.json  | write_file  ${BASEDIR}/${NETWORK_NAME}/nonces.json

# list headers to retrieve 2 headers for last snapshot
db-server query --query list-blocks \
          --config ${CONFIG_DIR}/configs/config.json \
          --db ${CHAIN_DB_DIR} |  jq -rc "[ .[] | select(.slot <= $last_snapshot) ] | .[0:2] | .[] | [.slot, .hash] | @csv" > ${BASEDIR}/${NETWORK_NAME}/headers.csv

# list headers to retrieve 2 headers for second to last snapshot
# this is necessary because when epoch transition happens, we compute
# the new active nonce for the epoch using the tail's parent hash!
db-server query --query list-blocks \
          --config ${CONFIG_DIR}/configs/config.json \
          --db ${CHAIN_DB_DIR} | jq -rc "[ .[] | select(.slot <= $second_to_last_snapshot) ] | .[0:2] | .[] | [.slot, .hash] | @csv" >> ${BASEDIR}/${NETWORK_NAME}/headers.csv

# retrieve actual headers content
mkdir  ${BASEDIR}/${NETWORK_NAME}/headers
cat ${BASEDIR}/${NETWORK_NAME}/headers.csv | tr -d '"' | while IFS=, read -ra hdr ; do
    db-server query --query "get-header ${hdr[0]}.${hdr[1]}" \
              --config ${CONFIG_DIR}/configs/config.json \
              --db ${CHAIN_DB_DIR} >  "${BASEDIR}/${NETWORK_NAME}/headers/header.${hdr[0]}.${hdr[1]}.cbor"
done

# import ledger state
amaru import-ledger-state --network ${NETWORK_NAME} --ledger-dir ${BASEDIR}/ledger.${NETWORK_NAME}.db --snapshot-dir ${BASEDIR}/${NETWORK_NAME}/snapshots/

# import headers
amaru import-headers --network ${NETWORK_NAME} --chain-dir ${BASEDIR}/chain.${NETWORK_NAME}.db --config-dir ${BASEDIR}/

# import nonces
amaru import-nonces  --nonces-file ${BASEDIR}/${NETWORK_NAME}/nonces.json --network ${NETWORK_NAME} --chain-dir ${BASEDIR}/chain.${NETWORK_NAME}.db/

nodes=$(ls -d /state/*)
number_of_nodes=$(ls -d /state/* | wc -l)
echo "number_of_nodes: $number_of_nodes"
for node in $nodes; do
  node_ix=$(echo "$node" | awk -F '/' '{print $3}')
  echo "configure node: $node ($node_ix)"
  copy_databases "$node_ix"
done
