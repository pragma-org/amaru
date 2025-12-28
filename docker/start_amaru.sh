#!/usr/bin/env bash

set -Eeuo pipefail

usage() {
    echo "Usage: $0" >&2
    echo "$*" >&2
    exit 1
}

DATA_DIR="/srv/amaru"
AMARU_PEER_ADDRESS="${AMARU_PEER_ADDRESS:-cardano:3001}"
AMARU_LEDGER_DIR="${AMARU_LEDGER_DIR:-${DATA_DIR}/ledger.db}"
AMARU_CHAIN_DIR="${AMARU_CHAIN_DIR:-${DATA_DIR}/chain.db}"

[[ -z "${AMARU_NETWORK:-}" ]] && usage "Set AMARU_NETWORK (via .env, compose, or shell) to a value supported by Amaru"

if ! [ -d "${AMARU_LEDGER_DIR}" ]
then
    cargo run --profile dev -- bootstrap \
      --ledger-dir "${AMARU_LEDGER_DIR}" \
      --chain-dir "${AMARU_CHAIN_DIR}"
fi

# keep stack traces for troubleshooting purposes
export RUST_BACKTRACE=full

export AMARU_LOG=gasket=error,amaru=debug,amaru::stages::consensus::forward_chain=info,info

exec cargo run --profile dev -- run \
      --peer-address "${AMARU_PEER_ADDRESS}" \
      --ledger-dir "${AMARU_LEDGER_DIR}" \
      --chain-dir "${AMARU_CHAIN_DIR}"
