#!/usr/bin/env bash

exitWithUsage () {
  echo -e "\033[1;31mError: missing argument(s)!\033[00m\n"
  echo -e "\033[1;32mUsage:\033[00m\n    $0 PEER_ADDRESS LISTEN_ADDRESS TARGET_EPOCH [NETWORK]\n"
  echo -e "\033[1mExamples:\033[00m \n    $0 127.0.0.1:3000 0.0.0.0:0    173\n    $0 127.0.0.1:3001 0.0.0.0:8000 200 preprod"
  exit 1
}

PEER_ADDRESS=$1
if [ -z "$PEER_ADDRESS" ]; then
  exitWithUsage
fi

LISTEN_ADDRESS=$2
if [ -z "$LISTEN_ADDRESS" ]; then
  exitWithUsage
fi

TARGET_EPOCH=$3
if [ -z "$TARGET_EPOCH" ]; then
  exitWithUsage
fi

NETWORK=${4:-preprod}

LEDGER_DIR=${LEDGER_DIR:-./ledger.db}

CHAIN_DIR=${CHAIN_DIR:-./chain.db}

echo -e "      \033[1;32mTarget\033[00m epoch $TARGET_EPOCH"
set -eo pipefail
AMARU_TRACE="amaru=info" cargo run --release -- --with-json-traces daemon \
           --peer-address="${PEER_ADDRESS}" \
           --listen-address="${LISTEN_ADDRESS}" \
           --network="${NETWORK}" \
           --chain-dir="${CHAIN_DIR}" \
           --ledger-dir="${LEDGER_DIR}" | while read line; do
  EVENT=$(jq -r '.fields.message' <<< "$line" 2>/dev/null)
  SPAN=$(jq -r '.span.name' <<< "$line" 2>/dev/null)
  if [ "$EVENT" = "exit" ] && [ "$SPAN" = "epoch_transition" ]; then
    EPOCH=$(jq -r '.span.into' <<< "$line" 2>/dev/null)
    if [ "$EPOCH" -eq "$TARGET_EPOCH" ]; then
      echo "Target epoch reached, stopping the process."
      pkill -INT -P $$
      break
    fi
  else
    LEVEL=$(jq -r '.level' <<< "$line" 2>/dev/null)
    if [ "$LEVEL" = "ERROR" ]; then
      # Sometimes the process doesn't fully properly exits
      echo "Got an error, force-stopping the process."
      pkill -INT -P $$
      break
    fi
  fi
done
