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
    # No configuration to consult, which is the case before setup has downloaded it and for good in
    # public-upstream mode, where nothing needs it. Mainnet is the only network taking --mainnet
    # instead of a magic, so its name is enough to decide; assuming a magic here instead would build
    # `--testnet-magic` with no argument, since mainnet has no magic to look up.
    [[ "$NETWORK" != mainnet ]]
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

official_cardano_node_config_base_url() {
  case "$NETWORK" in
    mainnet | preprod | preview) echo "https://book.world.dev.cardano.org/environments/$NETWORK" ;;
    *) return 1 ;;
  esac
}

download_official_cardano_node_config_file() {
  local base_url="$1" file_name="$2" target_dir="$3" target
  target="$target_dir/$file_name"
  mkdir -p "$(dirname "$target")"
  curl -fsSL "$base_url/$file_name" -o "$target"
}

cardano_node_config_referenced_files() {
  jq -r '
    [
      .AlonzoGenesisFile,
      .ByronGenesisFile,
      .CheckpointsFile,
      .ConwayGenesisFile,
      .ShelleyGenesisFile
    ]
    | map(select(. != null and . != ""))
    | unique
    | .[]
  ' "$1"
}

cardano_node_config_complete() {
  local file_name files

  [[ -f "$(cardano_node_config_file)" && -f "$(cardano_node_topology_file)" ]] || return 1
  have jq || return 0

  files="$(cardano_node_config_referenced_files "$(cardano_node_config_file)")" || return 1
  while IFS= read -r file_name; do
    [[ -n "$file_name" ]] || continue
    [[ -f "$CARDANO_NODE_CONFIG_DIR/$file_name" ]] || return 1
  done <<< "$files"
}

download_official_cardano_node_config() {
  local base_url tmp_dir file_name files

  if cardano_node_config_complete; then
    echo "[setup] using existing cardano-node config in $CARDANO_NODE_CONFIG_DIR"
    return
  fi

  base_url="$(official_cardano_node_config_base_url || true)"
  [[ -n "$base_url" ]] ||
    die "CARDANO_NODE_CONFIG_DIR does not exist: $CARDANO_NODE_CONFIG_DIR; automatic config download is only supported for mainnet, preprod, and preview"
  have curl || die "curl not found; cannot download cardano-node config"
  have jq || die "jq not found; cannot inspect cardano-node config"

  tmp_dir="$CARDANO_NODE_CONFIG_DIR.download.$$"
  rm -rf "$tmp_dir"
  mkdir -p "$tmp_dir"

  echo "[setup] downloading cardano-node config for $NETWORK"
  download_official_cardano_node_config_file "$base_url" "config.json" "$tmp_dir"
  download_official_cardano_node_config_file "$base_url" "topology.json" "$tmp_dir"

  files="$(cardano_node_config_referenced_files "$tmp_dir/config.json")"

  while IFS= read -r file_name; do
    [[ -n "$file_name" ]] || continue
    download_official_cardano_node_config_file "$base_url" "$file_name" "$tmp_dir"
  done <<< "$files"

  mkdir -p "$CARDANO_NODE_CONFIG_DIR"
  find "$tmp_dir" -type f | while IFS= read -r file_name; do
    local relative target
    relative="${file_name#"$tmp_dir"/}"
    target="$CARDANO_NODE_CONFIG_DIR/$relative"
    mkdir -p "$(dirname "$target")"
    mv "$file_name" "$target"
  done
  rm -rf "$tmp_dir"
  echo "[setup] cardano-node config ready in $CARDANO_NODE_CONFIG_DIR"
}

cardano_node_socket_file() {
  echo "$CARDANO_NODE_SOCKET_FILE"
}

cardano_node_database_dir() {
  echo "${CARDANO_NODE_DB:-$CARDANO_NODE_CONFIG_DIR/db}"
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

managed_cardano_node_stopped() {
  [[ -n "${CARDANO_NODE_PID:-}" ]] && ! kill -0 "$CARDANO_NODE_PID" 2>/dev/null
}

cardano_node_stopped_error() {
  local phase="$1" log_hint=""
  if [[ -n "${CARDANO_NODE_LOG_FILE:-}" ]]; then
    log_hint="; see $CARDANO_NODE_LOG_FILE"
  fi
  die "cardano-node stopped $phase$log_hint"
}

# Waits until the configured Cardano node socket is available.
wait_for_cardano_socket() {
  local socket
  socket="$(cardano_node_socket_file)"
  local timeout="${CARDANO_NODE_SOCKET_TIMEOUT_SECONDS:-1800}"
  for (( elapsed = 0; elapsed < timeout; elapsed++ )); do
    [[ -S "$socket" ]] && return 0
    managed_cardano_node_stopped && cardano_node_stopped_error "before creating its socket"
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
    managed_cardano_node_stopped && cardano_node_stopped_error "before answering local queries"
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
    managed_cardano_node_stopped && cardano_node_stopped_error "before reaching the requested sync progress"
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

# Parses the JSON returned by `query tx-mempool tx-exists`, verifies that it describes the expected
# transaction, and prints either "present" or "absent". Any schema or transaction-id mismatch is an
# error rather than an implicit absence.
parse_cardano_node_mempool_tx_state() {
  local expected_tx_id="$1" response_file="$2"

  jq -er --arg expected_tx_id "$expected_tx_id" '
    if type != "object"
      or (.txId | type) != "string"
      or (.txId | ascii_downcase) != ($expected_tx_id | ascii_downcase)
      or (.exists | type) != "boolean"
    then error("unexpected tx-mempool response")
    elif .exists then "present"
    else "absent"
    end
  ' "$response_file"
}

# Queries whether a transaction has diffused into the connected cardano-node mempool.
cardano_node_mempool_tx_state() {
  local tx_id="$1" response_file="${2:-}"
  local temporary_response=false result

  if [[ -z "$response_file" ]]; then
    response_file="${TMPDIR:-/tmp}/cardano-node-mempool-$$.json"
    temporary_response=true
  fi

  if ! "$CARDANO_CLI" conway query tx-mempool \
    $(cardano_cli_network_args) \
    --socket-path "$(cardano_node_socket_file)" \
    tx-exists "$tx_id" \
    --output-json >"$response_file"; then
    [[ "$temporary_response" == false ]] || rm -f "$response_file"
    return 1
  fi

  if parse_cardano_node_mempool_tx_state "$tx_id" "$response_file"; then
    result=0
  else
    result=$?
  fi
  [[ "$temporary_response" == false ]] || rm -f "$response_file"
  return "$result"
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

  if have sha256sum; then
    checksum_tool=sha256sum
  elif have shasum; then
    checksum_tool=shasum
  else
    die "neither sha256sum nor shasum found; cannot verify cardano-node $CARDANO_NODE_RELEASE_VERSION checksum"
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

  codesign --force --sign - "$CARDANO_NODE_HOME/bin/cardano-node" "$CARDANO_NODE_HOME/bin/cardano-cli"
}

# Returns whether setup must provide the pinned cardano-node release, which only local upstream
# mode needs: it is the one that runs cardano-node itself. Public mode signs transactions with
# cardano-cli, and that comes from its own much smaller release.
cardano_node_tools_needed() {
  ! public_cardano_upstream_enabled
}

setup() {
  require_unscaled_process setup
  download_official_cardano_node_config
  if tx_generation_enabled; then
    ensure_cardano_cli
  fi
  if ! cardano_node_tools_needed; then
    echo "[setup] cardano-node not needed (public upstream); skipping its release download"
    return 0
  fi
  if [[ "$CARDANO_NODE_HOME_WAS_SET" == true ]]; then
    echo "[setup] CARDANO_NODE_HOME is set: $CARDANO_NODE_HOME"
  elif [[ -x "$CARDANO_NODE_HOME/bin/cardano-node" && -x "$CARDANO_NODE_HOME/bin/cardano-cli" ]]; then
    echo "[setup] using existing cardano-node tools in $CARDANO_NODE_HOME"
  else
    download_cardano_node_home
  fi

  if ! public_cardano_upstream_enabled; then
    [[ -x "$CARDANO_NODE" ]] || die "cardano-node not found at $CARDANO_NODE"
  fi
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
    --database-path "$(cardano_node_database_dir)" \
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
