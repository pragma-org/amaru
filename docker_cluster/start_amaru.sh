#!/usr/bin/env bash

usage() {
    echo "Usage: $0" >&2
    echo "$*" >&2
    exit 1
}

PEER_ADDRESS="${PEER_ADDRESS:-cardano:3001}"
LEDGER_DIR="${LEDGER_DIR:-ledger.db}"
CONFIG_FOLDER="${CONFIG_FOLDER:-data}"
CHAIN_DIR="${CHAIN_DIR:-chain.db}"

[[ -z "$NETWORK" ]] && usage "Define NETWORK in .env with a value supported by Amaru"

if ! [ -d $LEDGER_DIR ]
then
    cargo run --profile dev -- bootstrap \
      --config-dir "${CONFIG_FOLDER}" \
      --ledger-dir "${LEDGER_DIR}" \
      --chain-dir "${CHAIN_DIR}" \
      --network "${NETWORK}"
fi

cargo run --profile dev -- daemon \
      --peer-address "${PEER_ADDRESS}" \
      --ledger-dir "${LEDGER_DIR}" \
      --chain-dir "${CHAIN_DIR}" \
      --network "${NETWORK}"
