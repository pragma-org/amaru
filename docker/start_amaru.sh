#!/usr/bin/env bash

set -Eeuo pipefail

usage() {
    echo "Usage: $0" >&2
    echo "$*" >&2
    exit 1
}

DATA_DIR="/srv/amaru"
AMARU_PEER="${AMARU_PEER:-cardano:3001}"
AMARU_LEDGER_DB="${AMARU_LEDGER_DB:-${DATA_DIR}/ledger.db}"
AMARU_CHAIN_DB="${AMARU_CHAIN_DB:-${DATA_DIR}/chain.db}"

[[ -z "${AMARU_NETWORK:-}" ]] && usage "Set AMARU_NETWORK (via .env, compose, or shell) to a value supported by Amaru"

if ! [ -d "${AMARU_LEDGER_DB}" ]
then
    cargo run --profile dev -- node bootstrap \
      --ledger-db "${AMARU_LEDGER_DB}" \
      --chain-db "${AMARU_CHAIN_DB}"
fi

# keep stack traces for troubleshooting purposes
export RUST_BACKTRACE=full

export AMARU_LOG=amaru=debug,amaru::stages::consensus::forward_chain=info,info

exec cargo run --profile dev -- node run \
      --peer "${AMARU_PEER}" \
      --ledger-db "${AMARU_LEDGER_DB}" \
      --chain-db "${AMARU_CHAIN_DB}"
