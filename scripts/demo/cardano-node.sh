#!/usr/bin/env bash

# Validates CARDANO_NODE points to an executable file.
require_cardano_node() {
  [[ -n "$CARDANO_NODE" ]] || die "CARDANO_NODE must be set (path to cardano-node executable)"
  [[ -f "$CARDANO_NODE" ]] || die "CARDANO_NODE must point to the executable file, not a directory: $CARDANO_NODE"
  [[ -x "$CARDANO_NODE" ]] || die "CARDANO_NODE is not executable: $CARDANO_NODE"
}

# Validates CARDANO_CLI points to an executable file.
require_cardano_cli() {
  [[ -n "$CARDANO_CLI" ]] || die "CARDANO_CLI must be set or available on PATH"
  [[ -x "$CARDANO_CLI" ]] || die "CARDANO_CLI is not executable: $CARDANO_CLI"
}

# Resolves the testnet magic from CARDANO_TESTNET_MAGIC, config.json, or a default.
network_magic() {
  if [[ -n "$CARDANO_TESTNET_MAGIC" ]]; then
    echo "$CARDANO_TESTNET_MAGIC"
    return
  fi

  if have jq && [[ -f "$(cardano_node_config_file)" ]]; then
    local config_file magic shelley_genesis_file
    config_file="$(cardano_node_config_file)"

    magic="$(jq -r '.NetworkMagic // empty' "$config_file")"
    if [[ -n "$magic" ]]; then
      echo "$magic"
      return
    fi

    shelley_genesis_file="$(jq -r '.ShelleyGenesisFile // empty' "$config_file")"
    if [[ -n "$shelley_genesis_file" ]]; then
      if [[ "$shelley_genesis_file" != /* ]]; then
        shelley_genesis_file="$CARDANO_NODE_CONFIG_DIR/$shelley_genesis_file"
      fi
      if [[ -f "$shelley_genesis_file" ]]; then
        magic="$(jq -r '.networkMagic // empty' "$shelley_genesis_file")"
        if [[ -n "$magic" ]]; then
          echo "$magic"
          return
        fi
      fi
    fi
  fi

  echo 1
}

cardano_node_config_file() {
  echo "${CARDANO_NODE_CONFIG_FILE:-$CARDANO_NODE_CONFIG_DIR/config.json}"
}

cardano_node_topology_file() {
  echo "${CARDANO_NODE_TOPOLOGY_FILE:-$CARDANO_NODE_CONFIG_DIR/topology.json}"
}

cardano_node_socket_file() {
  echo "${CARDANO_NODE_SOCKET_FILE:-$CARDANO_NODE_CONFIG_DIR/node.socket}"
}

prepare_cardano_node_topology_file() {
  local topology peer_snapshot snapshot generated
  topology="$(cardano_node_topology_file)"
  CARDANO_NODE_EFFECTIVE_TOPOLOGY_FILE="$topology"

  if ! have jq || [[ ! -f "$topology" ]]; then
    return
  fi

  peer_snapshot="$(jq -r '.peerSnapshotFile // empty' "$topology")"
  [[ -n "$peer_snapshot" ]] || return

  snapshot="$peer_snapshot"
  if [[ "$snapshot" != /* ]]; then
    snapshot="$CARDANO_NODE_CONFIG_DIR/$snapshot"
  fi
  [[ -f "$snapshot" ]] || return

  if jq -e 'any(.bigLedgerPools[]?.relays[]?; has("domain") and (has("address") | not))' "$snapshot" >/dev/null; then
    mkdir -p "$RUNDIR/generated"
    generated="$RUNDIR/generated/cardano-topology.json"
    jq 'del(.peerSnapshotFile)' "$topology" > "$generated"
    CARDANO_NODE_EFFECTIVE_TOPOLOGY_FILE="$generated"
    echo "[cardano-upstream] peer snapshot uses relay domain entries unsupported by this cardano-node; using $generated without peerSnapshotFile" >&2
  fi
}

cardano_node_effective_topology_file() {
  echo "${CARDANO_NODE_EFFECTIVE_TOPOLOGY_FILE:-$(cardano_node_topology_file)}"
}

# Waits until the configured Cardano node socket is available.
wait_for_cardano_socket() {
  local socket
  socket="$(cardano_node_socket_file)"
  local timeout="${CARDANO_NODE_SOCKET_TIMEOUT_SECONDS:-1800}"
  for (( elapsed = 0; elapsed < timeout; elapsed++ )); do
    [[ -S "$socket" ]] && return 0
    sleep 1
  done
  die "cardano-node socket not found: $socket"
}

wait_for_cardano_query() {
  local magic="$1"
  local timeout="${CARDANO_NODE_QUERY_TIMEOUT_SECONDS:-1800}"
  for (( elapsed = 0; elapsed < timeout; elapsed++ )); do
    if cardano_node_tip_slot "$magic" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  die "cardano-node socket did not answer local queries within ${timeout}s"
}

# Queries the current Cardano node tip slot.
cardano_node_tip_slot() {
  local socket
  socket="$(cardano_node_socket_file)"
  local magic="$1"
  "$CARDANO_CLI" conway query tip \
    --testnet-magic "$magic" \
    --socket-path "$socket" \
    | jq -r '.slot // empty'
}
