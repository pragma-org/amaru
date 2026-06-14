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
    fmt::{self, Display},
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    str::FromStr,
};

use amaru::{default_data_dir, default_snapshots_dir};
use amaru_kernel::{Epoch, HeaderHash, NetworkName, Point, Slot, utils};
use amaru_mithril::{
    chunk_for_slot, download_from_mithril, extract_block_header_cbor, first_missing_immutable_chunk,
    parse_header_slot_and_hash,
};
use anyhow::anyhow;
use clap::{ArgAction, Parser};
use num::{CheckedAdd, CheckedSub};
use serde::{Deserialize, Serialize};
use tracing::info;

mod archive;
mod config;
mod db_analyser;
mod koios;

use archive::{
    archive_path_for_target, existing_archive_paths, existing_snapshot_paths, materialize_snapshot,
    snapshot_path_for_target, write_epoch_metadata, write_snapshot_archive,
};
use config::resolve_config_dir;
use db_analyser::{ensure_db_analyser_binary, exact_snapshot_dir, run_db_analyser, select_analyse_from_slot};
use koios::{fetch_current_epoch, fetch_last_block_for_epoch};

const PACKAGED_HEADERS_FILE_NAME: &str = "bootstrap.headers.json";

#[derive(Debug, Parser)]
pub struct Args {
    /// The target network to choose from.
    #[arg(
        long,
        value_name = amaru::value_names::NETWORK,
        env = amaru::env_vars::NETWORK,
    )]
    network: NetworkName,

    /// The target epoch *after* bootstrap.
    ///
    /// The command expands it to the three consecutive snapshots required for bootstrap.
    ///
    /// If omitted, the current/latest network epoch will be resolved from an explorer and used as
    /// a target.
    #[arg(
        long,
        value_name = amaru::value_names::UINT,
        env = amaru::env_vars::EPOCH
    )]
    epoch: Option<Epoch>,

    /// Distribution directory used for metadata, caches and temporary work files.
    #[arg(
        long = "dist-dir",
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::DIST_DIR,
    )]
    dist_dir: Option<PathBuf>,

    /// Directory where snapshot archives and materialized snapshot directories are written.
    ///
    /// Defaults to ./snapshots/<NETWORK>/ when unspecified.
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::SNAPSHOTS_DIR,
    )]
    snapshot_dir: Option<PathBuf>,

    /// Forcefully erase requested generated snapshot outputs and regenerate them.
    #[arg(
        short,
        long,
        action = ArgAction::SetTrue,
        default_value_t = false,
    )]
    force: bool,

    /// Directory containing the cardano-node config.json and genesis files.
    ///
    /// Only required for custom testnet networks. For mainnet, preprod and preview,
    /// the config is downloaded automatically from the official source and cached
    /// when no local bundled copy is available.
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::CARDANO_NODE_CONFIG_DIR,
    )]
    cardano_node_config_dir: Option<PathBuf>,

    /// Use an existing local cardano-node database instead of downloading via Mithril.
    ///
    /// The directory must contain the cardano-node `immutable/` chunks covering all
    /// target slots (the standard chain-db layout). Required for custom networks,
    /// which have no Mithril aggregator; when the local chunks already cover the
    /// requested slots the Mithril download is skipped entirely.
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::CARDANO_NODE_DB,
    )]
    cardano_node_db: Option<PathBuf>,

    /// An optional list of three snapshot points. The option may be repeated.
    ///
    /// When omitted, the points are resolved from an explorer (Koios). When provided, they must
    /// correspond to the last point in an epoch, and its parent; separated by '::'.
    ///
    /// Amaru requires three snapshots to bootstrap. Hence, when used, this option must be repeated
    /// three times for each snapshot point.
    #[arg(
        long,
        value_name = amaru::value_names::SNAPSHOT,
        env = amaru::env_vars::SNAPSHOT,
        action = ArgAction::Append,
    )]
    snapshot: Vec<SnapshotPoint>,
}

#[derive(Debug, Clone)]
struct SnapshotPoint {
    point: Point,
    parent_point: Point,
}

impl Display for SnapshotPoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}::{}", &self.point, &self.parent_point)
    }
}

impl FromStr for SnapshotPoint {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut split = s.split("::");

        let point =
            split.next().ok_or_else(|| "missing snapshot point".to_string()).and_then(|s| s.parse::<Point>())?;

        let parent_point =
            split.next().ok_or_else(|| "missing parent snapshot point".to_string()).and_then(|s| s.parse::<Point>())?;

        Ok(Self { point, parent_point })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct EpochTarget {
    epoch: Epoch,
    slot: Slot,
    hash: HeaderHash,
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "header_parent")]
    parent_point: Option<Point>,
    #[serde(default)]
    archive_path: Option<String>,
    #[serde(default)]
    snapshot_path: Option<String>,
}

impl EpochTarget {
    pub fn from_snapshot_points(
        epoch: Epoch,
        mut snapshots: Vec<SnapshotPoint>,
    ) -> Result<Vec<Self>, Box<dyn std::error::Error>> {
        if snapshots.len() != 3 {
            return Err(anyhow!("expected exactly 3 snapshot points; got {}", snapshots.len()).into());
        }

        snapshots.sort_by_key(|s| std::cmp::Reverse(s.point.slot_or_default()));

        Ok(snapshots
            .into_iter()
            .enumerate()
            .map(|(ix, snapshot)| Self {
                epoch: epoch - Epoch::from(ix as u64 + 1),
                slot: snapshot.point.slot_or_default(),
                hash: snapshot.point.hash(),
                parent_point: Some(snapshot.parent_point),
                archive_path: None,
                snapshot_path: None,
            })
            .collect())
    }
}

fn default_dist_dir(network: NetworkName) -> PathBuf {
    repo_root().join(default_data_dir(network)).join("epoch-snapshots")
}

fn default_snapshot_output_dir(network: NetworkName) -> PathBuf {
    repo_root().join(default_snapshots_dir(network))
}

pub async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let Args {
        network,
        epoch,
        dist_dir,
        snapshot_dir,
        force,
        cardano_node_config_dir,
        cardano_node_db,
        snapshot: snapshot_points,
    } = args;

    let client = reqwest::Client::new();
    let dist_dir = dist_dir.unwrap_or_else(|| default_dist_dir(network));
    let metadata_dir = dist_dir.join("epochs");
    let snapshot_output_dir = snapshot_dir.unwrap_or_else(|| default_snapshot_output_dir(network));
    let work_dir = dist_dir.join("work");
    let cardano_node_db = cardano_node_db.unwrap_or_else(|| work_dir.join("cardano-db"));
    let ledger_snapshot_dir = cardano_node_db.join("ledger");
    let snapshots_str = utils::string::display_collection(&snapshot_points);

    fs::create_dir_all(&metadata_dir)?;
    fs::create_dir_all(&snapshot_output_dir)?;
    fs::create_dir_all(cardano_node_db.join("immutable"))?;
    fs::create_dir_all(&ledger_snapshot_dir)?;

    let config_dir = resolve_config_dir(&client, cardano_node_config_dir, network, &work_dir).await?;

    // Resolve the epoch targets: from an explicit targets file (Koios bypass, for custom
    // testnets) or from Koios (public networks).
    let mut targets = if snapshot_points.is_empty() {
        let start_epoch = resolve_start_epoch(&client, network, epoch).await?;
        let target_epochs = bootstrap_target_epochs(start_epoch)?;
        let mut resolved = Vec::with_capacity(target_epochs.len());

        for epoch in target_epochs {
            resolved.push(fetch_last_block_for_epoch(&client, network, epoch).await?);
        }

        resolved
    } else {
        EpochTarget::from_snapshot_points(
            epoch.ok_or(anyhow!("target epoch must be provided when using manual snapshot points."))?,
            snapshot_points,
        )?
    };

    for target in &mut targets {
        target.archive_path =
            Some(archive_path_for_target(&snapshot_output_dir, target).to_string_lossy().into_owned());
        target.snapshot_path =
            Some(snapshot_path_for_target(&snapshot_output_dir, target).to_string_lossy().into_owned());
    }
    targets.sort_unstable_by_key(|target| target.slot);

    // Fail fast: every target except the oldest must carry a parent_point, since
    // bootstrap packages headers for the 2nd and 3rd snapshots from it. Without
    // this, create-snapshots succeeds but bootstrap later fails for want of the
    // packaged bootstrap.headers.json.
    if let Some(target) = targets.iter().skip(1).find(|target| target.parent_point.is_none()) {
        return Err(format!(
            "target epoch {} (slot {}) is missing parent_point; required to package bootstrap headers",
            target.epoch, target.slot
        )
        .into());
    }

    info!(
        _command = "create-snapshots",
        snapshot_output_dir = %snapshot_output_dir.display(),
        config_dir = %config_dir.display(),
        cardano_node_db = %cardano_node_db.display(),
        network = %network,
        dist_dir = %dist_dir.display(),
        force,
        epoch = epoch
            .map(|e| Box::new(e.to_string()) as Box<dyn tracing::Value>)
            .unwrap_or_else(|| Box::new(tracing::field::Empty)),
        snapshots = snapshots_str,
        "running",
    );

    if force {
        remove_target_outputs(&snapshot_output_dir, &targets)?;
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

    for target in &targets {
        write_epoch_metadata(&metadata_dir, target)?;
    }

    let from_chunk = first_missing_immutable_chunk(&cardano_node_db.join("immutable"))?;
    let required_chunk = targets.last().map(|t| chunk_for_slot(network, t.slot.into())).unwrap_or(0);
    if from_chunk > required_chunk {
        info!(from_chunk, required_chunk, target_dir = %cardano_node_db.display(), "local cardano-db already covers all target slots; skipping Mithril download");
    } else {
        info!(from_chunk, target_dir = %cardano_node_db.display(), "synchronizing cardano-db from Mithril");
        download_from_mithril(network, cardano_node_db.clone(), from_chunk).await?;
    }

    let db_analyser_binary = ensure_db_analyser_binary()?;
    let immutable_dir = cardano_node_db.join("immutable");
    let context = SnapshotBuildContext {
        snapshot_output_dir: &snapshot_output_dir,
        immutable_dir: &immutable_dir,
        ledger_snapshot_dir: &ledger_snapshot_dir,
        metadata_dir: &metadata_dir,
        config_dir: &config_dir,
        cardano_node_db: &cardano_node_db,
        db_analyser_binary: &db_analyser_binary,
    };

    targets.into_iter().try_fold(None, |previous_snapshot_slot, target| {
        process_target(target, previous_snapshot_slot, &context).map(Some)
    })?;

    Ok(())
}

struct SnapshotBuildContext<'a> {
    snapshot_output_dir: &'a Path,
    immutable_dir: &'a Path,
    ledger_snapshot_dir: &'a Path,
    metadata_dir: &'a Path,
    config_dir: &'a Path,
    cardano_node_db: &'a Path,
    db_analyser_binary: &'a str,
}

fn process_target(
    mut target: EpochTarget,
    previous_snapshot_slot: Option<Slot>,
    context: &SnapshotBuildContext<'_>,
) -> Result<Slot, Box<dyn std::error::Error>> {
    let prepared_snapshot_path = snapshot_path_for_target(context.snapshot_output_dir, &target);
    let prepared_archive_path = archive_path_for_target(context.snapshot_output_dir, &target);
    let snapshot_dir =
        resolve_or_create_snapshot_dir(&target, previous_snapshot_slot, context.ledger_snapshot_dir, context)?;

    info!(epoch = %target.epoch, slot = %target.slot, snapshot = %prepared_snapshot_path.display(), "materializing bootstrap snapshot directory");
    materialize_snapshot(&snapshot_dir, &prepared_snapshot_path)?;
    write_packaged_headers(&target, context.immutable_dir, &prepared_snapshot_path)?;

    info!(epoch = %target.epoch, slot = %target.slot, archive = %prepared_archive_path.display(), "packaging snapshot archive");
    write_snapshot_archive(&prepared_snapshot_path, &prepared_archive_path)?;

    target.archive_path = Some(prepared_archive_path.to_string_lossy().into_owned());
    target.snapshot_path = Some(prepared_snapshot_path.to_string_lossy().into_owned());
    write_epoch_metadata(context.metadata_dir, &target)?;

    info!(epoch = %target.epoch, slot = %target.slot, snapshot = %prepared_snapshot_path.display(), archive = %prepared_archive_path.display(), "finished epoch snapshot");

    Ok(target.slot)
}

fn resolve_or_create_snapshot_dir(
    target: &EpochTarget,
    previous_snapshot_slot: Option<Slot>,
    ledger_snapshot_dir: &Path,
    context: &SnapshotBuildContext<'_>,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Some(snapshot_dir) = exact_snapshot_dir(ledger_snapshot_dir, target.slot) {
        info!(epoch = %target.epoch, slot = %target.slot, snapshot = %snapshot_dir.display(), "reusing existing db-analyser snapshot");
        return Ok(snapshot_dir);
    }

    let analyse_from = select_analyse_from_slot(ledger_snapshot_dir, target.slot, previous_snapshot_slot)?;

    info!(
        epoch = %target.epoch,
        slot = %target.slot,
        analyse_from = analyse_from.map(|s| Box::new(s.to_string()) as Box<dyn tracing::Value>).unwrap_or_else(|| Box::new(tracing::field::Empty)),
        "creating ledger snapshot with db-analyser"
    );

    run_db_analyser(
        context.db_analyser_binary,
        context.config_dir,
        context.cardano_node_db,
        target.slot,
        analyse_from,
    )?;

    exact_snapshot_dir(ledger_snapshot_dir, target.slot)
        .ok_or_else(|| format!("db-analyser did not create snapshot directory for slot {}", target.slot).into())
}

fn write_packaged_headers(
    target: &EpochTarget,
    immutable_dir: &Path,
    prepared_snapshot_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let packaged_headers = packaged_headers_for_target(immutable_dir, target.parent_point)?;
    if packaged_headers.is_empty() {
        return Ok(());
    }

    fs::write(prepared_snapshot_path.join(PACKAGED_HEADERS_FILE_NAME), serde_json::to_vec_pretty(&packaged_headers)?)?;

    Ok(())
}

async fn resolve_start_epoch(
    client: &reqwest::Client,
    network: NetworkName,
    requested_epoch: Option<Epoch>,
) -> Result<Epoch, Box<dyn std::error::Error>> {
    if let Some(epoch) = requested_epoch {
        return Ok(epoch.checked_sub(Epoch::THREE).ok_or_else(|| {
            anyhow!("epoch underflow: cannot bootstrap to the requested epoch: it is too early (must be >= 4).")
        })?);
    }

    let current_epoch = fetch_current_epoch(client, network).await?;
    infer_start_epoch(current_epoch)
}

fn infer_start_epoch(current_epoch: Epoch) -> Result<Epoch, Box<dyn std::error::Error>> {
    current_epoch
        .checked_sub(Epoch::THREE)
        .ok_or_else(|| format!("cannot infer bootstrap start epoch from current epoch {current_epoch}").into())
}

fn remove_target_outputs(
    snapshot_output_dir: &Path,
    targets: &[EpochTarget],
) -> Result<(), Box<dyn std::error::Error>> {
    for target in targets {
        remove_path_if_exists(&snapshot_path_for_target(snapshot_output_dir, target), "prepared snapshot directory")?;
        remove_path_if_exists(&archive_path_for_target(snapshot_output_dir, target), "prepared snapshot archive")?;
    }

    Ok(())
}

fn remove_path_if_exists(path: &Path, kind: &'static str) -> Result<(), Box<dyn std::error::Error>> {
    if !path.try_exists()? {
        return Ok(());
    }

    info!(path = %path.display(), kind, "removing existing create-snapshots output");

    if fs::symlink_metadata(path)?.is_dir() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }

    Ok(())
}

fn bootstrap_target_epochs(epoch: Epoch) -> Result<[Epoch; 3], Box<dyn std::error::Error>> {
    Ok([
        epoch,
        epoch
            .checked_add(Epoch::ONE)
            .ok_or_else(|| format!("bootstrap snapshot window overflows for epoch {epoch}"))?,
        epoch
            .checked_add(Epoch::TWO)
            .ok_or_else(|| format!("bootstrap snapshot window overflows for epoch {epoch}"))?,
    ])
}

pub(super) fn repo_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.parent().and_then(Path::parent).unwrap_or(manifest_dir.as_path()).to_path_buf()
}

// Extract exactly the two header CBORs that immediately follow the target parent point.
// It walks sorted immutable .chunk files using .secondary block offsets, matches the parent hash, then takes the next two headers.
fn packaged_headers_for_target(
    immutable_dir: &Path,
    parent_point: Option<Point>,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let Some(parent_point) = parent_point else {
        return Ok(Vec::new());
    };

    let parent_hash = hex::encode(parent_point.hash());

    let mut chunk_names = list_immutable_chunk_names(immutable_dir)?;
    chunk_names.sort_unstable();

    let entries = chunk_names
        .into_iter()
        .map(|chunk_name| read_chunk_header_entries(immutable_dir, &chunk_name))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();

    let Some(parent_index) = entries.iter().position(|(block_hash, _)| *block_hash == parent_hash) else {
        return Err(format!(
            "parent point {parent_point} not found in immutable blocks under {}",
            immutable_dir.display()
        )
        .into());
    };

    let headers =
        entries.into_iter().skip(parent_index + 1).map(|(_, header_cbor)| header_cbor).take(2).collect::<Vec<_>>();

    if headers.len() < 2 {
        return Err(format!(
            "could not package 2 bootstrap headers after parent point {parent_point} (found {} blocks)",
            headers.len()
        )
        .into());
    }

    Ok(headers)
}

fn read_chunk_header_entries(
    immutable_dir: &Path,
    chunk_name: &str,
) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    let chunk_path = immutable_dir.join(format!("{chunk_name}.chunk"));
    let secondary_path = immutable_dir.join(format!("{chunk_name}.secondary"));

    let offsets = read_secondary_offsets(&secondary_path)?;
    if offsets.is_empty() {
        return Ok(Vec::new());
    }

    let mut chunk_file = fs::File::open(&chunk_path)?;
    let chunk_len = chunk_file.metadata()?.len();

    let entries = offsets
        .iter()
        .copied()
        .enumerate()
        .map(|(idx, start)| read_chunk_header_entry(&mut chunk_file, &offsets, idx, start, chunk_len, &secondary_path))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect();

    Ok(entries)
}

fn read_chunk_header_entry(
    chunk_file: &mut fs::File,
    offsets: &[u64],
    idx: usize,
    start: u64,
    chunk_len: u64,
    secondary_path: &Path,
) -> Result<Option<(String, String)>, Box<dyn std::error::Error>> {
    let end = offsets.get(idx + 1).copied().unwrap_or(chunk_len);
    if end < start {
        return Err(format!(
            "invalid immutable offsets in {} at index {idx}: start={start}, end={end}",
            secondary_path.display()
        )
        .into());
    }

    let block_len = end - start;
    if block_len == 0 {
        return Ok(None);
    }

    chunk_file.seek(SeekFrom::Start(start))?;
    let mut block = vec![0u8; block_len as usize];
    chunk_file.read_exact(&mut block)?;

    let header = match parse_header_slot_and_hash(&block) {
        Ok(h) => h,
        Err(_) => return Ok(None),
    };

    let header_cbor = extract_block_header_cbor(&block)?;
    Ok(Some((hex::encode(header.header_hash), hex::encode(header_cbor))))
}

fn list_immutable_chunk_names(immutable_dir: &Path) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut names = Vec::new();
    for entry in fs::read_dir(immutable_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("chunk") {
            continue;
        }

        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };

        names.push(stem.to_string());
    }

    Ok(names)
}

fn read_secondary_offsets(secondary_path: &Path) -> Result<Vec<u64>, Box<dyn std::error::Error>> {
    const SECONDARY_ENTRY_SIZE: usize = 56;

    let secondary = fs::read(secondary_path)?;
    if secondary.len() % SECONDARY_ENTRY_SIZE != 0 {
        return Err(format!(
            "invalid immutable secondary index size for {}: {} bytes",
            secondary_path.display(),
            secondary.len()
        )
        .into());
    }

    let mut offsets = Vec::with_capacity(secondary.len() / SECONDARY_ENTRY_SIZE);
    for entry in secondary.chunks_exact(SECONDARY_ENTRY_SIZE) {
        let block_offset = u64::from_be_bytes(entry[0..8].try_into()?);
        offsets.push(block_offset);
    }

    Ok(offsets)
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use amaru_kernel::{Epoch, NetworkName, Slot, hash};
    use tempfile::TempDir;

    use super::{
        EpochTarget,
        archive::{
            archive_path_for_target, existing_archive_paths, existing_snapshot_paths, materialize_snapshot,
            snapshot_path_for_target, write_snapshot_archive,
        },
        bootstrap_target_epochs,
        db_analyser::{
            latest_snapshot_slot_at_or_before, parse_db_analyser_progress_line, parse_snapshot_slot_dir_name,
            select_analyse_from_slot,
        },
        default_snapshot_output_dir,
    };

    #[test]
    fn bootstrap_target_epochs_includes_three_consecutive_epochs() {
        assert_eq!(
            bootstrap_target_epochs(Epoch::from(163)).unwrap(),
            [Epoch::from(163), Epoch::from(164), Epoch::from(165)]
        );
    }

    #[test]
    fn bootstrap_target_epochs_rejects_overflow() {
        assert!(bootstrap_target_epochs(Epoch::from(u64::MAX)).is_err());
    }

    #[test]
    fn parse_snapshot_slot_dir_name_reads_expected_pattern() {
        assert_eq!(parse_snapshot_slot_dir_name("69206375_db-analyser"), Some(Slot::from(69_206_375)));
        assert_eq!(parse_snapshot_slot_dir_name("ledger"), None);
    }

    #[test]
    fn parse_db_analyser_progress_line_reads_elapsed_and_slot() {
        assert_eq!(
            parse_db_analyser_progress_line(
                "[176.010306s] BlockNo 873000      SlotNo 26757779     8bd0446350797fbd9a3592f74d717dea493874e1664a2be329b4eb23e8e165db"
            ),
            Some((176.010306, Slot::from(26_757_779)))
        );
    }

    #[test]
    fn latest_snapshot_slot_prefers_highest_slot_below_target() {
        let temp_dir = TempDir::new().unwrap();
        for slot in [100_u64, 150, 220] {
            fs::create_dir(temp_dir.path().join(format!("{slot}_db-analyser"))).unwrap();
        }

        assert_eq!(latest_snapshot_slot_at_or_before(temp_dir.path(), Slot::from(180)).unwrap(), Some(Slot::from(150)));
        assert_eq!(latest_snapshot_slot_at_or_before(temp_dir.path(), Slot::from(90)).unwrap(), None);
    }

    #[test]
    fn select_analyse_from_slot_prefers_previous_prepared_snapshot() {
        let temp_dir = TempDir::new().unwrap();
        for slot in [100_u64, 150, 220] {
            fs::create_dir(temp_dir.path().join(format!("{slot}_db-analyser"))).unwrap();
        }

        assert_eq!(
            select_analyse_from_slot(temp_dir.path(), Slot::from(220), Some(Slot::from(100))).unwrap(),
            Some(Slot::from(100))
        );
        assert!(select_analyse_from_slot(temp_dir.path(), Slot::from(220), Some(Slot::from(180))).is_err());
    }

    #[test]
    fn select_analyse_from_slot_falls_back_to_latest_existing_snapshot_for_first_target() {
        let temp_dir = TempDir::new().unwrap();
        for slot in [100_u64, 150, 220] {
            fs::create_dir(temp_dir.path().join(format!("{slot}_db-analyser"))).unwrap();
        }

        assert_eq!(select_analyse_from_slot(temp_dir.path(), Slot::from(200), None).unwrap(), Some(Slot::from(150)));
    }

    #[test]
    fn snapshot_path_uses_slot_and_hash() {
        let target = EpochTarget {
            epoch: Epoch::from(163),
            slot: Slot::from(69_206_375),
            hash: hash!("6f99b5f3deaeae8dc43fce3db2f3cd36ad8ed174ca3400b5b1bed76fdf248912"),
            parent_point: None,
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
            epoch: Epoch::from(163),
            slot: Slot::from(69_206_375),
            hash: hash!("6f99b5f3deaeae8dc43fce3db2f3cd36ad8ed174ca3400b5b1bed76fdf248912"),
            parent_point: None,
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
            epoch: Epoch::from(163),
            slot: Slot::from(69_206_375),
            hash: hash!("6f99b5f3deaeae8dc43fce3db2f3cd36ad8ed174ca3400b5b1bed76fdf248912"),
            parent_point: None,
            archive_path: None,
            snapshot_path: None,
        };
        let missing_target = EpochTarget {
            epoch: Epoch::from(164),
            slot: Slot::from(69_638_382),
            hash: hash!("5da6ba37a4a07df015c4ea92c880e3600d7f098b97e73816f8df04bbb5fad3b7"),
            parent_point: None,
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
            epoch: Epoch::from(163),
            slot: Slot::from(69_206_375),
            hash: hash!("6f99b5f3deaeae8dc43fce3db2f3cd36ad8ed174ca3400b5b1bed76fdf248912"),
            parent_point: None,
            archive_path: None,
            snapshot_path: None,
        };
        let missing_target = EpochTarget {
            epoch: Epoch::from(164),
            slot: Slot::from(69_638_382),
            hash: hash!("5da6ba37a4a07df015c4ea92c880e3600d7f098b97e73816f8df04bbb5fad3b7"),
            parent_point: None,
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
}
