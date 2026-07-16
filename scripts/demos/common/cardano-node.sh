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

# Reads the network magic declared by the cardano-node config or its Shelley genesis file.
config_network_magic() {
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
        jq -r '.networkMagic // empty' "$shelley_genesis_file"
      fi
    fi
  fi
}

# Resolves the network magic for AMARU_NETWORK, reading the cardano-node config for
# networks without a well-known magic.
network_magic() {
  local magic
  magic="$(expected_network_magic || true)"
  if [[ -z "$magic" ]]; then
    magic="$(config_network_magic)"
  fi
  [[ -n "$magic" ]] ||
    die "cannot determine the network magic for network $NETWORK; point CARDANO_NODE_CONFIG_DIR at a configuration declaring NetworkMagic or a Shelley genesis file"
  echo "$magic"
}

cardano_node_requires_network_magic() {
  if have jq && [[ -f "$(cardano_node_config_file)" ]]; then
    [[ "$(jq -r '.RequiresNetworkMagic // "RequiresMagic"' "$(cardano_node_config_file)")" != "RequiresNoMagic" ]]
  else
    return 0
  fi
}

cardano_cli_network_args() {
  if cardano_node_requires_network_magic; then
    printf '%s\n' --testnet-magic "$(network_magic)"
  else
    printf '%s\n' --mainnet
  fi
}

cardano_node_config_file() {
  echo "$CARDANO_NODE_CONFIG_DIR/config.json"
}

cardano_node_topology_file() {
  echo "$CARDANO_NODE_CONFIG_DIR/topology.json"
}

cardano_node_socket_file() {
  echo "$CARDANO_NODE_SOCKET_FILE"
}

cardano_node_bundled_peer_snapshot_file() {
  echo "$CARDANO_NODE_HOME/share/$NETWORK/peer-snapshot.json"
}

prepare_cardano_node_topology_file() {
  local topology peer_snapshot snapshot bundled_snapshot generated generated_snapshot
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
  bundled_snapshot="$(cardano_node_bundled_peer_snapshot_file)"

  if [[ ! -f "$snapshot" ]]; then
    if [[ -f "$bundled_snapshot" ]]; then
      mkdir -p "$RUNDIR/generated"
      generated="$RUNDIR/generated/cardano-topology.json"
      generated_snapshot="$RUNDIR/generated/cardano-peer-snapshot.json"
      cp "$bundled_snapshot" "$generated_snapshot"
      jq --arg peer_snapshot "$(basename "$generated_snapshot")" '.peerSnapshotFile = $peer_snapshot' "$topology" > "$generated"
      CARDANO_NODE_EFFECTIVE_TOPOLOGY_FILE="$generated"
      echo "[cardano-upstream] peer snapshot file not found: $snapshot; using bundled $bundled_snapshot" >&2
    else
      mkdir -p "$RUNDIR/generated"
      generated="$RUNDIR/generated/cardano-topology.json"
      jq 'del(.peerSnapshotFile)' "$topology" > "$generated"
      CARDANO_NODE_EFFECTIVE_TOPOLOGY_FILE="$generated"
      echo "[cardano-upstream] peer snapshot file not found: $snapshot; using $generated without peerSnapshotFile" >&2
    fi
    return
  fi

  if jq -e 'has("version") or any(.bigLedgerPools[]?.relays[]?; has("domain") and (has("address") | not))' "$snapshot" >/dev/null; then
    mkdir -p "$RUNDIR/generated"
    generated="$RUNDIR/generated/cardano-topology.json"
    if [[ -f "$bundled_snapshot" ]]; then
      generated_snapshot="$RUNDIR/generated/cardano-peer-snapshot.json"
      cp "$bundled_snapshot" "$generated_snapshot"
      jq --arg peer_snapshot "$(basename "$generated_snapshot")" '.peerSnapshotFile = $peer_snapshot' "$topology" > "$generated"
      CARDANO_NODE_EFFECTIVE_TOPOLOGY_FILE="$generated"
      echo "[cardano-upstream] peer snapshot is incompatible with this cardano-node; using bundled $bundled_snapshot" >&2
    else
      jq 'del(.peerSnapshotFile)' "$topology" > "$generated"
      CARDANO_NODE_EFFECTIVE_TOPOLOGY_FILE="$generated"
      echo "[cardano-upstream] peer snapshot is incompatible with this cardano-node; using $generated without peerSnapshotFile" >&2
    fi
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
  local timeout="${CARDANO_NODE_QUERY_TIMEOUT_SECONDS:-1800}"
  for (( elapsed = 0; elapsed < timeout; elapsed++ )); do
    if cardano_node_tip >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  die "cardano-node socket did not answer local queries within ${timeout}s"
}

wait_for_cardano_sync_progress() {
  local threshold="${1:-99.9}" timeout="${2:-14400}"
  local tip progress

  for (( elapsed = 0; elapsed < timeout; elapsed++ )); do
    if tip="$(cardano_node_tip 2>/dev/null)"; then
      progress="$(jq -r '.syncProgress // "0"' <<< "$tip")"
      if jq -en --arg progress "$progress" --argjson threshold "$threshold" '($progress | tonumber) >= $threshold' >/dev/null; then
        return 0
      fi
      if ((elapsed % 30 == 0)); then
        echo "[cardano-upstream] waiting for cardano-node sync progress: ${progress}%/${threshold}%"
      fi
    fi
    sleep 1
  done
  die "cardano-node did not reach ${threshold}% sync progress within ${timeout}s"
}

cardano_node_tip() {
  local socket
  socket="$(cardano_node_socket_file)"
  "$CARDANO_CLI" conway query tip \
    $(cardano_cli_network_args) \
    --socket-path "$socket"
}

# Queries the current Cardano node tip slot.
cardano_node_tip_slot() {
  cardano_node_tip | jq -r '.slot // empty'
}

# Returns whether the demo uses a public upstream peer instead of a local cardano-node.
public_cardano_upstream_enabled() {
  [[ "$CARDANO_UPSTREAM_MODE" == "public" ]]
}

expected_network_magic() {
  case "$NETWORK" in
    preprod) echo 1 ;;
    preview) echo 2 ;;
    *) return 1 ;;
  esac
}

# Validates that the configured cardano-node config matches AMARU_NETWORK.
validate_network_config() {
  local expected_magic actual_magic

  expected_magic="$(expected_network_magic || true)"
  [[ -n "$expected_magic" ]] || return 0

  actual_magic="$(network_magic)"
  if [[ "$actual_magic" != "$expected_magic" ]]; then
    die "AMARU_NETWORK=$NETWORK expects testnet magic $expected_magic, but CARDANO_NODE_CONFIG_DIR=$CARDANO_NODE_CONFIG_DIR reports magic $actual_magic"
  fi
}

cardano_node_archive_platform() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"

  case "$os:$arch" in
    Darwin:arm64) echo "macos-arm64" ;;
    Darwin:x86_64) echo "macos-amd64" ;;
    Linux:aarch64 | Linux:arm64) echo "linux-arm64" ;;
    Linux:x86_64) echo "linux-amd64" ;;
    *) die "unsupported platform for cardano-node release download: $os/$arch" ;;
  esac
}

# Downloads and unpacks the configured cardano-node release into CARDANO_NODE_HOME.
download_cardano_node_home() {
  local platform archive_name archive_path checksums_path expected actual checksum_tool
  local download_base_url="https://github.com/IntersectMBO/cardano-node/releases/download/$CARDANO_NODE_RELEASE_VERSION"

  platform="$(cardano_node_archive_platform)"
  archive_name="cardano-node-$CARDANO_NODE_RELEASE_VERSION-$platform.tar.gz"
  archive_path="$LOGDIR/$archive_name"
  checksums_path="$LOGDIR/cardano-node-$CARDANO_NODE_RELEASE_VERSION-sha256sums.txt"

  have curl || die "curl not found; cannot download cardano-node $CARDANO_NODE_RELEASE_VERSION"
  have tar || die "tar not found; cannot unpack cardano-node $CARDANO_NODE_RELEASE_VERSION"

  mkdir -p "$LOGDIR"
  if [[ ! -f "$archive_path" ]]; then
    echo "[setup] downloading $archive_name"
    curl -fL "$download_base_url/$archive_name" -o "$archive_path"
  else
    echo "[setup] using cached $archive_path"
  fi

  if have shasum; then
    checksum_tool=shasum
  elif have sha256sum; then
    checksum_tool=sha256sum
  else
    die "neither shasum nor sha256sum found; cannot verify cardano-node $CARDANO_NODE_RELEASE_VERSION checksum"
  fi
  if [[ ! -f "$checksums_path" ]]; then
    echo "[setup] downloading cardano-node checksums"
    curl -fsSL "$download_base_url/cardano-node-$CARDANO_NODE_RELEASE_VERSION-sha256sums.txt" -o "$checksums_path"
  fi
  expected="$(awk -v name="$archive_name" '$2 == name { print $1 }' "$checksums_path")"
  [[ -n "$expected" ]] || die "checksum for $archive_name not found in $checksums_path"
  if [[ "$checksum_tool" == shasum ]]; then
    actual="$(shasum -a 256 "$archive_path" | awk '{ print $1 }')"
  else
    actual="$(sha256sum "$archive_path" | awk '{ print $1 }')"
  fi
  [[ "$actual" == "$expected" ]] || die "checksum mismatch for $archive_path"

  rm -rf "$CARDANO_NODE_HOME.in-progress"
  mkdir -p "$CARDANO_NODE_HOME.in-progress"
  tar -xzf "$archive_path" -C "$CARDANO_NODE_HOME.in-progress"
  rm -rf "$CARDANO_NODE_HOME"
  mv "$CARDANO_NODE_HOME.in-progress" "$CARDANO_NODE_HOME"
}

# Ad-hoc signs freshly downloaded macOS binaries so Gatekeeper allows running them.
repair_downloaded_cardano_node_home() {
  if [[ "$(uname -s)" != Darwin || "$CARDANO_NODE_HOME_WAS_SET" == true ]]; then
    return
  fi

  have codesign || return

  if "$CARDANO_NODE" --version >/dev/null 2>&1; then
    return
  fi

  codesign --force --sign - "$CARDANO_NODE_HOME/bin/cardano-node" "$CARDANO_NODE_HOME/bin/db-analyser"
}

setup() {
  require_unscaled_process setup
  if [[ "$CARDANO_NODE_HOME_WAS_SET" == true ]]; then
    echo "[setup] CARDANO_NODE_HOME is set: $CARDANO_NODE_HOME"
  elif [[ -x "$CARDANO_NODE_HOME/bin/cardano-node" && -x "$CARDANO_NODE_HOME/bin/db-analyser" ]]; then
    echo "[setup] using existing cardano-node tools in $CARDANO_NODE_HOME"
  else
    download_cardano_node_home
  fi

  [[ -x "$CARDANO_NODE_HOME/bin/db-analyser" ]] || die "db-analyser not found at $CARDANO_NODE_HOME/bin/db-analyser"
  [[ -x "$CARDANO_NODE" ]] || die "cardano-node not found at $CARDANO_NODE"
  repair_downloaded_cardano_node_home
  echo "[setup] cardano-node tools ready in $CARDANO_NODE_HOME"
}

# Runs the local upstream cardano-node, listening on UPSTREAM_PORT.
run_cardano_upstream() {
  if public_cardano_upstream_enabled; then
    die "cardano-upstream is disabled because CARDANO_UPSTREAM_MODE=public"
  fi
  require_cardano_node
  validate_network_config
  prepare_cardano_node_topology_file
  mkdir -p "$(dirname "$(cardano_node_socket_file)")"
  rm -f "$(cardano_node_socket_file)"
  "$CARDANO_NODE" run \
    --config "$(cardano_node_config_file)" \
    --topology "$(cardano_node_effective_topology_file)" \
    --database-path "$CARDANO_NODE_CONFIG_DIR/db" \
    --socket-path "$(cardano_node_socket_file)" \
    --port "$UPSTREAM_PORT" \
    2>&1 | tee "$LOGDIR/cardano-upstream.log"
}

# Readiness probe: the local cardano-node answers tip queries on its socket.
ready_cardano_upstream() {
  require_cardano_cli
  have jq || exit 1
  [[ -S "$(cardano_node_socket_file)" ]] || exit 1
  cardano_node_tip_slot >/dev/null 2>&1
}
