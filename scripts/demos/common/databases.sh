#!/usr/bin/env bash

# Manages the demo databases: Mithril refreshes of the Amaru source databases, their
# synchronization into isolated per-node run directories, and the initialization of the
# local cardano-node immutable database.
#
# Callers must set DEMO_NAME (used in the marker file names recording which snapshot a
# database copy came from), MITHRIL_REFRESH_DIR, MITHRIL_REFRESH_LOG_FILE,
# MITHRIL_SNAPSHOTS_DIR, AMARU_CHAIN_SOURCE_DIR, AMARU_LEDGER_SOURCE_DIR,
# REFRESH_FROM_MITHRIL, CARDANO_NODE_INIT_FROM_MITHRIL, and define a validate_config
# function checked before a refresh.

validate_amaru_source_databases() {
  amaru_source_databases_ready ||
    die "refreshed databases are missing or incomplete in $MITHRIL_REFRESH_DIR; run ./process-compose.sh refresh first"
}

amaru_source_databases_ready() {
  [[ -d "$AMARU_CHAIN_SOURCE_DIR" && -d "$AMARU_LEDGER_SOURCE_DIR" && -f "$MITHRIL_REFRESH_DIR/.mithril-refresh.json" ]]
}

# Initializes the local cardano-node immutable database from the selected Mithril snapshot.
initialize_cardano_node_database() {
  local full_source_immutable="$AMARU_DIR/data/$NETWORK/epoch-snapshots/work/cardano-db/immutable"
  local source_immutable="$MITHRIL_SNAPSHOTS_DIR/$NETWORK/immutable"
  local target_db="$CARDANO_NODE_CONFIG_DIR/db"
  local target_marker="$target_db/.$DEMO_NAME-mithril-source.json"
  local source_marker="$MITHRIL_REFRESH_DIR/.mithril-refresh.json"

  case "$CARDANO_NODE_INIT_FROM_MITHRIL" in
    auto | true | TRUE | 1 | yes | YES | on | ON) ;;
    false | FALSE | 0 | no | NO | off | OFF)
      echo "[initialize] skipped cardano-node database initialization because CARDANO_NODE_INIT_FROM_MITHRIL=false"
      return 0
      ;;
    *) die "CARDANO_NODE_INIT_FROM_MITHRIL must be auto, true, or false" ;;
  esac

  public_cardano_upstream_enabled && return 0
  [[ -f "$source_marker" ]] || die "Mithril refresh metadata not found: $source_marker"
  have rsync || die "rsync not found; cannot initialize cardano-node database from Mithril"

  if [[ -f "$full_source_immutable/00000.primary" ]]; then
    source_immutable="$full_source_immutable"
  fi

  if [[ -f "$target_db/immutable/00000.primary" ]] && same_mithril_snapshot_metadata "$source_marker" "$target_marker"; then
    if ! truthy "$CARDANO_NODE_INIT_FROM_MITHRIL"; then
      ensure_cardano_node_db_marker "$target_db" "$(dirname "$source_immutable")"
      echo "[initialize] cardano-node database already initialized from selected Mithril snapshot; skipping sync"
      return 0
    fi
    echo "[initialize] CARDANO_NODE_INIT_FROM_MITHRIL=$CARDANO_NODE_INIT_FROM_MITHRIL; re-initializing cardano-node database from the selected Mithril snapshot"
  fi

  [[ -d "$source_immutable" ]] || die "Mithril immutable files not found: $source_immutable"
  [[ -f "$source_immutable/00000.primary" ]] ||
    die "cardano-node immutable source is partial: $source_immutable; expected 00000.primary so cardano-node can replay ledger from genesis"

  echo "[initialize] initializing cardano-node database from Mithril immutable files..."
  mkdir -p "$target_db"
  rm -rf "$target_db/ledger" "$target_db/volatile"
  mkdir -p "$target_db/immutable"
  rsync -a --delete "$source_immutable"/ "$target_db/immutable"/
  ensure_cardano_node_db_marker "$target_db" "$(dirname "$source_immutable")"
  cp "$source_marker" "$target_marker"
  echo "[initialize] cardano-node immutable database initialized from $source_immutable"
  echo "[initialize] cardano-node will rebuild ledger and volatile state on next start"
}

# cardano-node refuses to open a non-empty database directory that lacks the protocolMagicId
# marker file it normally writes itself (NoDbMarkerAndNotEmpty), so provide one when only the
# immutable files were synchronized.
ensure_cardano_node_db_marker() {
  local target_db="$1" source_root="$2"
  if [[ -f "$source_root/clean" && ! -f "$target_db/clean" ]]; then
    cp "$source_root/clean" "$target_db/clean"
  fi
  [[ -f "$target_db/protocolMagicId" ]] && return 0
  if [[ -f "$source_root/protocolMagicId" ]]; then
    cp "$source_root/protocolMagicId" "$target_db/protocolMagicId"
  else
    jq -j '.networkMagic' "$CARDANO_NODE_CONFIG_DIR/shelley-genesis.json" > "$target_db/protocolMagicId"
  fi
}

same_mithril_snapshot_metadata() {
  local source="$1" target="$2"
  [[ -f "$source" && -f "$target" ]] || return 1
  jq -e -s '.[0].network == .[1].network and .[0].snapshot.hash == .[1].snapshot.hash' "$source" "$target" >/dev/null
}

# Synchronizes a source database into an isolated run directory, skipping the sync when
# the destination still matches the refreshed snapshot marker.
sync_database_dir() {
  local label="$1" source="$2" destination="$3"
  local source_marker="$MITHRIL_REFRESH_DIR/.mithril-refresh.json"
  local destination_marker="$destination/.$DEMO_NAME-source.json"
  if [[ -f "$destination_marker" ]] && cmp -s "$source_marker" "$destination_marker"; then
    echo "[initialize] $label database unchanged; skipping sync"
    return 0
  fi

  echo "[initialize] synchronizing $label database: $source -> $destination"
  mkdir -p "$destination"
  rsync -a --delete "$source"/ "$destination"/
  cp "$source_marker" "$destination_marker"
}

mark_database_dir_dirty() {
  local directory="$1"
  rm -f "$directory/.$DEMO_NAME-source.json"
}

refresh_from_mithril() {
  require_db_analyser_with_analyse_from
  cd "$AMARU_DIR" || return
  mkdir -p "$(dirname "$MITHRIL_REFRESH_LOG_FILE")"
  AMARU_NETWORK="$NETWORK" \
    BUILD_PROFILE="$BUILD_PROFILE" \
    CARDANO_NODE_HOME="$CARDANO_NODE_HOME" \
    STAGING_DIR="$MITHRIL_REFRESH_DIR" \
    AMARU_MITHRIL_SNAPSHOTS_DIR="$MITHRIL_SNAPSHOTS_DIR" \
    INSTALL=false \
    FORCE_REFRESH="${FORCE_REFRESH:-false}" \
    ./scripts/refresh-from-mithril \
    2>&1 | tee "$MITHRIL_REFRESH_LOG_FILE"
}

# Refreshes the Amaru source databases from Mithril according to REFRESH_FROM_MITHRIL.
run_mithril_refresh() {
  setup
  validate_config
  case "$REFRESH_FROM_MITHRIL" in
    auto)
      if truthy "${FORCE_REFRESH:-false}"; then
        echo "[mithril-refresh] FORCE_REFRESH=true; refreshing from Mithril"
      elif amaru_source_databases_ready; then
        echo "[mithril-refresh] using existing refreshed databases from $MITHRIL_REFRESH_DIR"
        return 0
      else
        echo "[mithril-refresh] refreshed databases missing or incomplete; refreshing from Mithril"
      fi
      ;;
    true | TRUE | 1 | yes | YES | on | ON) ;;
    false | FALSE | 0 | no | NO | off | OFF)
      echo "[mithril-refresh] skipped because REFRESH_FROM_MITHRIL=false"
      return 0
      ;;
    *) die "REFRESH_FROM_MITHRIL must be auto, true, or false" ;;
  esac

  if ! refresh_from_mithril; then
    if amaru_source_databases_ready; then
      echo "[mithril-refresh] refresh failed; using existing refreshed databases from $MITHRIL_REFRESH_DIR"
    else
      die "Mithril refresh failed and no usable refreshed databases exist in $MITHRIL_REFRESH_DIR; see $MITHRIL_REFRESH_LOG_FILE"
    fi
  fi
}
