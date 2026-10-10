#!/usr/bin/env bash

set -Eeuo pipefail

usage() {
    echo "Usage: $0" >&2
    echo "$*" >&2
    exit 1
}

DATA_DIR="/srv/amaru"
AMARU_PEER="${AMARU_PEER:-cardano:3001}"
AMARU_DB_LEDGER="${AMARU_DB_LEDGER:-${DATA_DIR}/ledger.db}"
AMARU_DB_CHAIN="${AMARU_DB_CHAIN:-${DATA_DIR}/chain.db}"

[[ -z "${AMARU_NETWORK:-}" ]] && usage "Set AMARU_NETWORK (via .env, compose, or shell) to a value supported by Amaru"

if ! [ -d "${AMARU_DB_LEDGER}" ]
then
    cargo run --profile dev -- node bootstrap \
      --db-ledger "${AMARU_DB_LEDGER}" \
      --db-chain "${AMARU_DB_CHAIN}"
fi

# keep stack traces for troubleshooting purposes
export RUST_BACKTRACE=full

export AMARU_LOG=amaru=debug,amaru::stages::consensus::forward_chain=info,info

exec cargo run --profile dev -- node run \
      --peer "${AMARU_PEER}" \
      --db-ledger "${AMARU_DB_LEDGER}" \
      --db-chain "${AMARU_DB_CHAIN}"
