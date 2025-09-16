#!/usr/bin/env bash

set -Eeuo pipefail

usage() {
    echo "Usage: $0" >&2
    echo "$*" >&2
    exit 1
}

DATA_DIR="/srv/amaru"
PEER_ADDRESS="${PEER_ADDRESS:-cardano:3001}"
LEDGER_DIR="${LEDGER_DIR:-${DATA_DIR}/ledger.db}"
CHAIN_DIR="${CHAIN_DIR:-${DATA_DIR}/chain.db}"
CONFIG_FOLDER="${CONFIG_FOLDER:-data}"

[[ -z "${AMARU_NETWORK:-}" ]] && usage "Set AMARU_NETWORK (via .env, compose, or shell) to a value supported by Amaru"

if ! [ -d "${LEDGER_DIR}" ]
then
    cargo run --profile dev -- bootstrap \
      --config-dir "${CONFIG_FOLDER}" \
      --ledger-dir "${LEDGER_DIR}" \
      --chain-dir "${CHAIN_DIR}"
fi

exec cargo run --profile dev -- daemon \
      --peer-address "${PEER_ADDRESS}" \
      --ledger-dir "${LEDGER_DIR}" \
      --chain-dir "${CHAIN_DIR}"
