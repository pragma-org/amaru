#!/usr/bin/env bash

# Manages the demo databases: bootstrapping the Amaru source databases from the public
# snapshot CDN with `amaru node bootstrap`, and their synchronization into isolated
# per-node run directories.
#
# Callers must set DEMO_NAME (used in the marker file names recording which bootstrap a
# database copy came from), BOOTSTRAP_AMARU_DATABASES, AMARU_BOOTSTRAP_DIR,
# AMARU_BOOTSTRAP_LOG_FILE, AMARU_CHAIN_SOURCE_DIR, and AMARU_LEDGER_SOURCE_DIR.

validate_amaru_source_databases() {
  amaru_source_databases_ready ||
    die "bootstrapped databases are missing or incomplete in $AMARU_BOOTSTRAP_DIR; run ./process-compose.sh refresh first"
}

amaru_source_databases_ready() {
  [[ -d "$AMARU_CHAIN_SOURCE_DIR" && -d "$AMARU_LEDGER_SOURCE_DIR" && -f "$(bootstrap_marker_file)" ]]
}

# Records which bootstrap produced the source databases; written only after a successful
# bootstrap, so an interrupted run leaves no marker and is retried on the next start.
bootstrap_marker_file() {
  echo "$AMARU_BOOTSTRAP_DIR/.bootstrap.json"
}

# Synchronizes a source database into an isolated run directory, skipping the sync when
# the destination was initialized from the current bootstrap. The marker records source
# provenance while the node advances the working database beyond that bootstrap point.
sync_database_dir() {
  local label="$1" source="$2" destination="$3"
  local source_marker destination_marker legacy_destination_marker
  source_marker="$(bootstrap_marker_file)"
  destination_marker="${destination%/}.$DEMO_NAME-source.json"
  legacy_destination_marker="${destination%/}/.$DEMO_NAME-source.json"

  # Older demos stored this marker inside the RocksDB directory. Move it beside
  # the database so RocksDB does not report it as an unexpected snapshot file.
  if [[ -f "$legacy_destination_marker" ]]; then
    mv "$legacy_destination_marker" "$destination_marker"
  fi

  if [[ -f "$destination_marker" ]] && cmp -s "$source_marker" "$destination_marker"; then
    echo "[initialize] $label database unchanged; skipping sync"
    return 0
  fi

  echo "[initialize] synchronizing $label database: $source -> $destination"
  mkdir -p "$destination"
  rsync -a --delete "$source"/ "$destination"/
  cp "$source_marker" "$destination_marker"
}

# Bootstraps the Amaru source databases from the public snapshot CDN. The chain and ledger
# directories are recreated from scratch (`amaru node bootstrap` refuses populated
# directories), but the downloaded snapshot archives under $AMARU_BOOTSTRAP_DIR/snapshots
# are kept and reused across runs.
bootstrap_amaru_databases() {
  ensure_amaru_node_binary
  mkdir -p "$AMARU_BOOTSTRAP_DIR" "$(dirname "$AMARU_BOOTSTRAP_LOG_FILE")"
  rm -rf "$AMARU_CHAIN_SOURCE_DIR" "$AMARU_LEDGER_SOURCE_DIR"
  rm -f "$(bootstrap_marker_file)"
  # The snapshot download cache lands in snapshots/<network> relative to the working
  # directory, so run the bootstrap from inside $AMARU_BOOTSTRAP_DIR. The explicit
  # --chain-dir/--ledger-dir arguments shield the bootstrap from any AMARU_CHAIN_DIR or
  # AMARU_LEDGER_DIR exported in the caller's environment.
  (
    cd "$AMARU_BOOTSTRAP_DIR" &&
      "$(amaru_node_binary)" node bootstrap \
        --network "$NETWORK" \
        --chain-dir "$AMARU_CHAIN_SOURCE_DIR" \
        --ledger-dir "$AMARU_LEDGER_SOURCE_DIR" \
        ${AMARU_BOOTSTRAP_EPOCH:+--epoch "$AMARU_BOOTSTRAP_EPOCH"}
  ) 2>&1 | tee "$AMARU_BOOTSTRAP_LOG_FILE"
  jq -n \
    --arg network "$NETWORK" \
    --arg epoch "${AMARU_BOOTSTRAP_EPOCH:-latest}" \
    --arg completed_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    '{network: $network, epoch: $epoch, completed_at: $completed_at}' \
    >"$(bootstrap_marker_file)"
}

# Bootstraps the Amaru source databases according to BOOTSTRAP_AMARU_DATABASES; in auto
# mode existing databases are reused so restarting the demo never re-bootstraps.
run_bootstrap() {
  case "$BOOTSTRAP_AMARU_DATABASES" in
    auto)
      if truthy "${FORCE_REFRESH:-false}"; then
        echo "[bootstrap] FORCE_REFRESH=true; bootstrapping from the snapshot CDN"
      elif amaru_source_databases_ready; then
        echo "[bootstrap] using existing bootstrapped databases from $AMARU_BOOTSTRAP_DIR"
        return 0
      else
        echo "[bootstrap] bootstrapped databases missing or incomplete; bootstrapping from the snapshot CDN"
      fi
      ;;
    true | TRUE | 1 | yes | YES | on | ON) ;;
    false | FALSE | 0 | no | NO | off | OFF)
      echo "[bootstrap] skipped because BOOTSTRAP_AMARU_DATABASES=false"
      return 0
      ;;
    *) die "BOOTSTRAP_AMARU_DATABASES must be auto, true, or false" ;;
  esac

  bootstrap_amaru_databases
}
