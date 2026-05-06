// Copyright 2026 PRAGMA
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::{
    fs,
    path::{Path, PathBuf},
};

use amaru::{DEFAULT_NETWORK, default_data_dir, default_snapshots_dir};
use amaru_kernel::NetworkName;
use clap::Parser;
use serde::{Deserialize, Serialize};
use tracing::info;

mod archive;
mod config;
mod db_analyser;
mod koios;
mod mithril;

use archive::{
    archive_path_for_target, existing_archive_paths, existing_snapshot_paths, materialize_snapshot,
    snapshot_path_for_target, write_epoch_metadata, write_snapshot_archive,
};
use config::{resolve_config_dir, resolve_db_analyser_build_config};
use db_analyser::{ensure_db_analyser_image, exact_snapshot_dir, run_db_analyser, select_analyse_from_slot};
use koios::fetch_last_block_for_epoch;
use mithril::{download_from_mithril, get_latest_chunk};

#[derive(Debug, Parser)]
pub struct Args {
    /// The target network to choose from.
    #[arg(
        long,
        value_name = amaru::value_names::NETWORK,
        env = amaru::env_vars::NETWORK,
        default_value_t = DEFAULT_NETWORK,
    )]
    network: NetworkName,

    /// The bootstrap start epoch to capture.
    ///
    /// The command expands it to the three consecutive snapshots bootstrap needs.
    #[arg(
        long = "epoch",
        required = true,
        value_name = amaru::value_names::UINT,
        env = amaru::env_vars::EPOCH,
    )]
    epoch: u64,

    /// Distribution directory used for metadata, caches and temporary work files.
    #[arg(
        long = "dist-dir",
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::DIST_DIR,
    )]
    dist_dir: Option<PathBuf>,

    /// Directory containing the cardano-node config.json and genesis files.
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::CARDANO_NODE_CONFIG_DIR,
    )]
    cardano_node_config_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct EpochTarget {
    epoch: u64,
    slot: u64,
    hash: String,
    archive_path: Option<String>,
    snapshot_path: Option<String>,
}

fn default_dist_dir(network: NetworkName) -> PathBuf {
    repo_root().join(default_data_dir(network)).join("epoch-snapshots")
}

fn default_snapshot_output_dir(network: NetworkName) -> PathBuf {
    repo_root().join(default_snapshots_dir(network))
}

pub async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let Args { network, epoch, dist_dir, cardano_node_config_dir } = args;
    let target_epochs = bootstrap_target_epochs(epoch)?;
    let dist_dir = dist_dir.unwrap_or_else(|| default_dist_dir(network));
    let metadata_dir = dist_dir.join("epochs");
    let snapshot_output_dir = default_snapshot_output_dir(network);
    let work_dir = dist_dir.join("work");
    let cardano_db_dir = work_dir.join("cardano-db");
    let ledger_snapshot_dir = cardano_db_dir.join("ledger");

    fs::create_dir_all(&metadata_dir)?;
    fs::create_dir_all(&snapshot_output_dir)?;
    fs::create_dir_all(cardano_db_dir.join("immutable"))?;
    fs::create_dir_all(&ledger_snapshot_dir)?;

    let client = reqwest::Client::new();
    let config_dir = resolve_config_dir(&client, cardano_node_config_dir, network, &work_dir).await?;

    info!(
        _command = "generate-epoch-snapshots",
        snapshot_output_dir = %snapshot_output_dir.display(),
        config_dir = %config_dir.display(),
        network = %network,
        dist_dir = %dist_dir.display(),
        requested_epoch = epoch,
        target_epochs = ?target_epochs,
        "running",
    );

    let mut targets = Vec::with_capacity(target_epochs.len());
    for epoch in target_epochs {
        let mut target = fetch_last_block_for_epoch(&client, network, epoch).await?;
        target.archive_path =
            Some(archive_path_for_target(&snapshot_output_dir, &target).to_string_lossy().into_owned());
        target.snapshot_path =
            Some(snapshot_path_for_target(&snapshot_output_dir, &target).to_string_lossy().into_owned());
        targets.push(target);
    }

    let existing_snapshots = existing_snapshot_paths(&snapshot_output_dir, &targets);
    let existing_archives = existing_archive_paths(&snapshot_output_dir, &targets);
    if !existing_snapshots.is_empty() || !existing_archives.is_empty() {
        let existing_outputs = existing_snapshots
            .into_iter()
            .chain(existing_archives)
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!("refusing to overwrite existing snapshot outputs: {existing_outputs}").into());
    }

    let mut pending_targets = targets;
    pending_targets.sort_unstable_by_key(|target| target.slot);

    for target in &pending_targets {
        write_epoch_metadata(&metadata_dir, target)?;
    }

    let latest_chunk = get_latest_chunk(&cardano_db_dir.join("immutable"))?;
    let from_chunk = latest_chunk.unwrap_or(0);
    info!(from_chunk, target_dir = %cardano_db_dir.display(), "synchronizing cardano-db from Mithril");
    download_from_mithril(network, cardano_db_dir.clone(), from_chunk).await?;

    let db_analyser_build_config = resolve_db_analyser_build_config()?;
    let db_analyser_image = ensure_db_analyser_image(&db_analyser_build_config)?;
    let mut previous_snapshot_slot = None;

    for mut target in pending_targets {
        let prepared_snapshot_path = snapshot_path_for_target(&snapshot_output_dir, &target);
        let prepared_archive_path = archive_path_for_target(&snapshot_output_dir, &target);
        let snapshot_dir = match exact_snapshot_dir(&ledger_snapshot_dir, target.slot) {
            Some(snapshot_dir) => {
                info!(epoch = target.epoch, slot = target.slot, snapshot = %snapshot_dir.display(), "reusing existing db-analyser snapshot");
                snapshot_dir
            }
            None => {
                let analyse_from = select_analyse_from_slot(&ledger_snapshot_dir, target.slot, previous_snapshot_slot)?;
                info!(
                    epoch = target.epoch,
                    slot = target.slot,
                    analyse_from,
                    "creating ledger snapshot with db-analyser"
                );
                run_db_analyser(&db_analyser_image, &config_dir, &cardano_db_dir, target.slot, analyse_from)?;
                exact_snapshot_dir(&ledger_snapshot_dir, target.slot)
                    .ok_or_else(|| format!("db-analyser did not create snapshot directory for slot {}", target.slot))?
            }
        };
        previous_snapshot_slot = Some(target.slot);

        info!(epoch = target.epoch, slot = target.slot, snapshot = %prepared_snapshot_path.display(), "materializing bootstrap snapshot directory");
        materialize_snapshot(&snapshot_dir, &prepared_snapshot_path)?;
        info!(epoch = target.epoch, slot = target.slot, archive = %prepared_archive_path.display(), "packaging snapshot archive");
        write_snapshot_archive(&prepared_snapshot_path, &prepared_archive_path)?;

        target.archive_path = Some(prepared_archive_path.to_string_lossy().into_owned());
        target.snapshot_path = Some(prepared_snapshot_path.to_string_lossy().into_owned());
        write_epoch_metadata(&metadata_dir, &target)?;

        info!(epoch = target.epoch, slot = target.slot, snapshot = %prepared_snapshot_path.display(), archive = %prepared_archive_path.display(), "finished epoch snapshot");
    }

    Ok(())
}

fn bootstrap_target_epochs(epoch: u64) -> Result<[u64; 3], Box<dyn std::error::Error>> {
    Ok([
        epoch,
        epoch.checked_add(1).ok_or_else(|| format!("bootstrap snapshot window overflows for epoch {epoch}"))?,
        epoch.checked_add(2).ok_or_else(|| format!("bootstrap snapshot window overflows for epoch {epoch}"))?,
    ])
}

pub(super) fn repo_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.parent().and_then(Path::parent).unwrap_or(manifest_dir.as_path()).to_path_buf()
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, fs, path::Path};

    use amaru_kernel::NetworkName;
    use tempfile::TempDir;

    use super::{
        EpochTarget,
        archive::{
            archive_path_for_target, existing_archive_paths, existing_snapshot_paths, materialize_snapshot,
            snapshot_path_for_target, write_snapshot_archive,
        },
        bootstrap_target_epochs,
        config::{CardanoNodeConfigManifest, dotenv_value, strip_optional_quotes},
        db_analyser::{
            DbAnalyserLogAction, DbAnalyserLogRelay, latest_snapshot_slot_at_or_before,
            parse_db_analyser_progress_line, parse_snapshot_slot_dir_name, sanitize_image_tag,
            select_analyse_from_slot,
        },
        default_snapshot_output_dir,
    };

    #[test]
    fn bootstrap_target_epochs_includes_three_consecutive_epochs() {
        assert_eq!(bootstrap_target_epochs(163).unwrap(), [163, 164, 165]);
    }

    #[test]
    fn bootstrap_target_epochs_rejects_overflow() {
        assert!(bootstrap_target_epochs(u64::MAX).is_err());
    }

    #[test]
    fn parse_snapshot_slot_dir_name_reads_expected_pattern() {
        assert_eq!(parse_snapshot_slot_dir_name("69206375_db-analyser"), Some(69_206_375));
        assert_eq!(parse_snapshot_slot_dir_name("ledger"), None);
    }

    #[test]
    fn parse_db_analyser_progress_line_reads_elapsed_and_slot() {
        assert_eq!(
            parse_db_analyser_progress_line(
                "[176.010306s] BlockNo 873000      SlotNo 26757779     8bd0446350797fbd9a3592f74d717dea493874e1664a2be329b4eb23e8e165db"
            ),
            Some((176.010306, 26_757_779))
        );
    }

    #[test]
    fn db_analyser_log_relay_reports_throttled_progress_with_eta() {
        let mut relay = DbAnalyserLogRelay::new(300, Some(100));

        assert_eq!(
            relay.handle_line("[0.100000s] Started StoreLedgerStateAt (SlotNo 300) LedgerReapply"),
            DbAnalyserLogAction::Report("db-analyser started: replaying from slot 100 to slot 300".to_string())
        );

        let first_report = relay.handle_line("[10.000000s] BlockNo 1      SlotNo 150     deadbeef");
        let DbAnalyserLogAction::Report(first_report) = first_report else {
            panic!("expected first db-analyser progress line to be reported");
        };
        assert!(first_report.contains("25.0%"));
        assert!(first_report.contains("eta 30s"));

        assert_eq!(
            relay.handle_line("[20.000000s] BlockNo 2      SlotNo 200     deadbeef"),
            DbAnalyserLogAction::Suppress
        );

        let second_report = relay.handle_line("[40.000000s] BlockNo 3      SlotNo 250     deadbeef");
        let DbAnalyserLogAction::Report(second_report) = second_report else {
            panic!("expected throttled db-analyser progress line to be reported after 30 seconds");
        };
        assert!(second_report.contains("75.0%"));
        assert!(second_report.contains("eta 13s"));
    }

    #[test]
    fn latest_snapshot_slot_prefers_highest_slot_below_target() {
        let temp_dir = TempDir::new().unwrap();
        for slot in [100_u64, 150, 220] {
            fs::create_dir(temp_dir.path().join(format!("{slot}_db-analyser"))).unwrap();
        }

        assert_eq!(latest_snapshot_slot_at_or_before(temp_dir.path(), 180).unwrap(), Some(150));
        assert_eq!(latest_snapshot_slot_at_or_before(temp_dir.path(), 90).unwrap(), None);
    }

    #[test]
    fn select_analyse_from_slot_prefers_previous_prepared_snapshot() {
        let temp_dir = TempDir::new().unwrap();
        for slot in [100_u64, 150, 220] {
            fs::create_dir(temp_dir.path().join(format!("{slot}_db-analyser"))).unwrap();
        }

        assert_eq!(select_analyse_from_slot(temp_dir.path(), 220, Some(100)).unwrap(), Some(100));
        assert!(select_analyse_from_slot(temp_dir.path(), 220, Some(180)).is_err());
    }

    #[test]
    fn select_analyse_from_slot_falls_back_to_latest_existing_snapshot_for_first_target() {
        let temp_dir = TempDir::new().unwrap();
        for slot in [100_u64, 150, 220] {
            fs::create_dir(temp_dir.path().join(format!("{slot}_db-analyser"))).unwrap();
        }

        assert_eq!(select_analyse_from_slot(temp_dir.path(), 200, None).unwrap(), Some(150));
    }

    #[test]
    fn snapshot_path_uses_slot_and_hash() {
        let target = EpochTarget {
            epoch: 163,
            slot: 69_206_375,
            hash: "6f99b5f3deaeae8dc43fce3db2f3cd36ad8ed174ca3400b5b1bed76fdf248912".to_string(),
            archive_path: None,
            snapshot_path: None,
        };
        let snapshot = snapshot_path_for_target(Path::new("snapshots/preprod"), &target);

        assert_eq!(
            snapshot,
            Path::new("snapshots/preprod/69206375.6f99b5f3deaeae8dc43fce3db2f3cd36ad8ed174ca3400b5b1bed76fdf248912")
        );
    }

    #[test]
    fn archive_path_uses_snapshot_name() {
        let target = EpochTarget {
            epoch: 163,
            slot: 69_206_375,
            hash: "6f99b5f3deaeae8dc43fce3db2f3cd36ad8ed174ca3400b5b1bed76fdf248912".to_string(),
            archive_path: None,
            snapshot_path: None,
        };
        let archive = archive_path_for_target(Path::new("snapshots/preprod"), &target);

        assert_eq!(
            archive,
            Path::new(
                "snapshots/preprod/69206375.6f99b5f3deaeae8dc43fce3db2f3cd36ad8ed174ca3400b5b1bed76fdf248912.tar.gz"
            )
        );
    }

    #[test]
    fn default_snapshot_output_dir_uses_snapshots_network_dir() {
        assert_eq!(default_snapshot_output_dir(NetworkName::Preprod), super::repo_root().join("snapshots/preprod"));
    }

    #[test]
    fn existing_snapshot_paths_returns_existing_requested_directories() {
        let temp_dir = TempDir::new().unwrap();
        let existing_target = EpochTarget {
            epoch: 163,
            slot: 69_206_375,
            hash: "6f99b5f3deaeae8dc43fce3db2f3cd36ad8ed174ca3400b5b1bed76fdf248912".to_string(),
            archive_path: None,
            snapshot_path: None,
        };
        let missing_target = EpochTarget {
            epoch: 164,
            slot: 69_638_382,
            hash: "5da6ba37a4a07df015c4ea92c880e3600d7f098b97e73816f8df04bbb5fad3b7".to_string(),
            archive_path: None,
            snapshot_path: None,
        };

        fs::create_dir(snapshot_path_for_target(temp_dir.path(), &existing_target)).unwrap();

        assert_eq!(
            existing_snapshot_paths(temp_dir.path(), &[existing_target.clone(), missing_target]),
            vec![snapshot_path_for_target(temp_dir.path(), &existing_target)]
        );
    }

    #[test]
    fn existing_archive_paths_returns_existing_requested_archives() {
        let temp_dir = TempDir::new().unwrap();
        let existing_target = EpochTarget {
            epoch: 163,
            slot: 69_206_375,
            hash: "6f99b5f3deaeae8dc43fce3db2f3cd36ad8ed174ca3400b5b1bed76fdf248912".to_string(),
            archive_path: None,
            snapshot_path: None,
        };
        let missing_target = EpochTarget {
            epoch: 164,
            slot: 69_638_382,
            hash: "5da6ba37a4a07df015c4ea92c880e3600d7f098b97e73816f8df04bbb5fad3b7".to_string(),
            archive_path: None,
            snapshot_path: None,
        };

        fs::write(archive_path_for_target(temp_dir.path(), &existing_target), []).unwrap();

        assert_eq!(
            existing_archive_paths(temp_dir.path(), &[existing_target.clone(), missing_target]),
            vec![archive_path_for_target(temp_dir.path(), &existing_target)]
        );
    }

    #[test]
    fn materialize_snapshot_converts_flat_tables_file_to_bootstrap_directory_shape() {
        let temp_dir = TempDir::new().unwrap();
        let source = temp_dir.path().join("69206375_db-analyser");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("state"), b"state").unwrap();
        fs::write(source.join("tables"), b"utxo").unwrap();

        let target = temp_dir.path().join("69206375.6f99b5f3deaeae8dc43fce3db2f3cd36ad8ed174ca3400b5b1bed76fdf248912");

        materialize_snapshot(&source, &target).unwrap();

        assert!(target.join("state").is_file());
        assert!(target.join("tables").join("tvar").is_file());
    }

    #[test]
    fn write_snapshot_archive_packages_materialized_directory() {
        let temp_dir = TempDir::new().unwrap();
        let snapshot_dir =
            temp_dir.path().join("69206375.6f99b5f3deaeae8dc43fce3db2f3cd36ad8ed174ca3400b5b1bed76fdf248912");
        fs::create_dir_all(snapshot_dir.join("tables")).unwrap();
        fs::write(snapshot_dir.join("state"), b"state").unwrap();
        fs::write(snapshot_dir.join("tables").join("tvar"), b"utxo").unwrap();

        let archive_path =
            temp_dir.path().join("69206375.6f99b5f3deaeae8dc43fce3db2f3cd36ad8ed174ca3400b5b1bed76fdf248912.tar.gz");

        write_snapshot_archive(&snapshot_dir, &archive_path).unwrap();

        assert!(archive_path.is_file());
    }

    #[test]
    fn sanitize_image_tag_only_keeps_docker_safe_characters() {
        let sanitized = sanitize_image_tag("refs/heads/main");
        assert_eq!(sanitized, "refs-heads-main");
    }

    #[test]
    fn config_manifest_collects_all_referenced_files() {
        let manifest = serde_json::from_str::<CardanoNodeConfigManifest>(
            r#"{
                "AlonzoGenesisFile": "alonzo-genesis.json",
                "ByronGenesisFile": "byron-genesis.json",
                "CheckpointsFile": "checkpoints.json",
                "ConwayGenesisFile": "conway-genesis.json",
                "ShelleyGenesisFile": "shelley-genesis.json"
            }"#,
        )
        .unwrap();

        assert_eq!(
            manifest.referenced_files(),
            vec![
                "alonzo-genesis.json",
                "byron-genesis.json",
                "checkpoints.json",
                "conway-genesis.json",
                "shelley-genesis.json"
            ]
        );
    }

    #[test]
    fn dotenv_value_prefers_first_present_key() {
        let values = BTreeMap::from([
            ("DB_ANALYSER_IMAGE".to_string(), "custom-image".to_string()),
            ("AMARU_DB_ANALYSER_IMAGE".to_string(), "legacy-image".to_string()),
        ]);

        assert_eq!(
            dotenv_value(&values, &["DB_ANALYSER_IMAGE", "AMARU_DB_ANALYSER_IMAGE"]),
            Some("custom-image".to_string())
        );
    }

    #[test]
    fn strip_optional_quotes_removes_matching_wrappers() {
        assert_eq!(strip_optional_quotes("\"value\""), "value");
        assert_eq!(strip_optional_quotes("'value'"), "value");
        assert_eq!(strip_optional_quotes("value"), "value");
    }
}
