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
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
};

use amaru::default_snapshots_dir;
use amaru_kernel::{
    BlockHeader, Epoch, HeaderHash, IsHeader, NetworkName, Point, Slot, from_cbor,
    num::{CheckedAdd, CheckedSub},
    utils::{self, path::relative_path},
};
use amaru_mithril::{
    ImmutableBlock, chunk_for_slot, download_from_mithril, first_missing_immutable_chunk, iter_immutable_blocks,
};
use amaru_observability::info;
use amaru_progress_bar::{ProgressBar, TerminalProgressBar};
use anyhow::anyhow;
use clap::{ArgAction, Parser};
use serde::{Deserialize, Serialize};

mod archive;
mod config;
mod db_analyser;
mod koios;

use amaru::lifecycle::{Runnable, RuntimeKind};
use archive::{archive_path_for_target, materialize_snapshot, snapshot_path_for_target, write_snapshot_archive};
use config::resolve_config_dir;
use db_analyser::{ensure_db_analyser_binary, exact_snapshot_dir, run_db_analyser, select_analyse_from_slot};
use koios::{fetch_current_epoch, fetch_last_block_for_epoch};

const PACKAGED_BLOCKS_FILE_NAME: &str = "bootstrap.blocks.json";

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
            })
            .collect())
    }
}

fn default_dist_dir(network: NetworkName) -> PathBuf {
    repo_root().join(format!("data/{}", network.to_string().to_lowercase())).join("epoch-snapshots")
}

pub(super) fn default_snapshot_output_dir(network: NetworkName) -> PathBuf {
    repo_root().join(default_snapshots_dir(network))
}

pub(crate) fn runnable(args: Args) -> Runnable {
    Runnable::exit_on_signal(RuntimeKind::Io, move || run(args))
}

async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let Args {
        network,
        epoch,
        dist_dir,
        snapshot_dir,
        cardano_node_config_dir,
        cardano_node_db,
        snapshot: snapshot_points,
    } = args;

    let client = reqwest::Client::new();
    let dist_dir = dist_dir.unwrap_or_else(|| default_dist_dir(network));
    let snapshot_output_dir = snapshot_dir.unwrap_or_else(|| default_snapshot_output_dir(network));
    let work_dir = dist_dir.join("work");
    let cardano_node_db = cardano_node_db.unwrap_or_else(|| work_dir.join("cardano-db"));
    let ledger_snapshot_dir = cardano_node_db.join("ledger");
    let snapshots_str = utils::string::display_collection(&snapshot_points);

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

    targets.sort_unstable_by_key(|target| target.slot);

    // Fail fast: every target except the oldest must carry a parent_point so its packaged block can
    // be checked against the expected chain.
    if let Some(target) = targets.iter().skip(1).find(|target| target.parent_point.is_none()) {
        return Err(format!(
            "target epoch {} (slot {}) is missing parent_point; required to verify its packaged bootstrap block",
            target.epoch, target.slot
        )
        .into());
    }

    info!(
        cli::snapshot::CREATE,
        snapshot_output_dir = %relative_path(&snapshot_output_dir)?.display(),
        config_dir = %relative_path(&config_dir)?.display(),
        cardano_node_db = %relative_path(&cardano_node_db)?.display(),
        network = %network,
        dist_dir = %relative_path(&dist_dir)?.display(),
        epoch = @epoch.map(|e| e.to_string()),
        snapshots = @(!snapshots_str.is_empty()).then_some(snapshots_str),
    );

    let from_chunk = first_missing_immutable_chunk(&cardano_node_db.join("immutable"))?;
    let required_chunk = targets.last().and_then(|t| chunk_for_slot(network, t.slot.into()).ok()).unwrap_or(0);

    let progress_factory: Arc<dyn Fn(usize, &str) -> Box<dyn ProgressBar + Send + Sync> + Send + Sync> =
        Arc::new(|size: usize, template: &str| {
            Box::new(TerminalProgressBar::new(size as u64, template)) as Box<dyn ProgressBar + Send + Sync>
        });

    if from_chunk > required_chunk {
        info!(
            cli::mithril::SKIP_DOWNLOAD,
            from_chunk,
            required_chunk,
            target_dir = %relative_path(&cardano_node_db)?.display(),
            reason = "cardano-node db already covers all target slots",
        );
    } else {
        info!(cli::mithril::DOWNLOAD, from_chunk, target_dir = %relative_path(&cardano_node_db)?.display());
        download_from_mithril(network, cardano_node_db.clone(), from_chunk, progress_factory.clone()).await?;
    }

    let db_analyser_binary = ensure_db_analyser_binary()?;
    let immutable_dir = cardano_node_db.join("immutable");
    let context = SnapshotBuildContext {
        snapshot_output_dir: &snapshot_output_dir,
        immutable_dir: &immutable_dir,
        ledger_snapshot_dir: &ledger_snapshot_dir,
        config_dir: &config_dir,
        cardano_node_db: &cardano_node_db,
        db_analyser_binary: &db_analyser_binary,
        with_progress: &progress_factory,
    };

    let mut previous_snapshot_slot = None;
    for target in &targets {
        previous_snapshot_slot = Some(process_target(target, previous_snapshot_slot, &context)?);
    }

    Ok(())
}

struct SnapshotBuildContext<'a> {
    snapshot_output_dir: &'a Path,
    immutable_dir: &'a Path,
    ledger_snapshot_dir: &'a Path,
    config_dir: &'a Path,
    cardano_node_db: &'a Path,
    db_analyser_binary: &'a str,
    with_progress: &'a Arc<dyn Fn(usize, &str) -> Box<dyn ProgressBar + Send + Sync> + Send + Sync>,
}

fn process_target(
    target: &EpochTarget,
    previous_snapshot_slot: Option<Slot>,
    context: &SnapshotBuildContext<'_>,
) -> Result<Slot, Box<dyn std::error::Error>> {
    let prepared_snapshot_path = snapshot_path_for_target(context.snapshot_output_dir, target);
    let prepared_archive_path = archive_path_for_target(context.snapshot_output_dir, target);

    if prepared_archive_path.is_file() {
        info!(
            cli::snapshot::SKIP_PACKAGE,
            epoch = %target.epoch,
            slot = %target.slot,
            archive = %relative_path(&prepared_archive_path)?.display(),
            reason = "already exists",
        );
        return Ok(target.slot);
    }

    if !prepared_snapshot_path.exists() {
        let snapshot_dir =
            resolve_or_create_snapshot_dir(target, previous_snapshot_slot, context.ledger_snapshot_dir, context)?;

        info!(
            cli::snapshot::MATERIALIZE,
            epoch = %target.epoch,
            slot = %target.slot,
            snapshot = %relative_path(&prepared_snapshot_path)?.display(),
        );
        materialize_snapshot(&snapshot_dir, &prepared_snapshot_path)?;
    } else {
        info!(
            cli::snapshot::SKIP_MATERIALIZE,
            epoch = %target.epoch,
            slot = %target.slot,
            snapshot = %relative_path(&prepared_snapshot_path)?.display(),
            reason = "already exists",
        );
    }

    write_packaged_blocks(target, context.immutable_dir, &prepared_snapshot_path)?;

    info!(
        cli::snapshot::PACKAGE,
        epoch = %target.epoch,
        slot = %target.slot,
        archive = %relative_path(&prepared_archive_path)?.display(),
    );
    write_snapshot_archive(&prepared_snapshot_path, &prepared_archive_path)?;

    info!(
        cli::snapshot::CREATED,
        epoch = %target.epoch,
        slot = %target.slot,
        archive = %relative_path(&prepared_archive_path)?.display(),
    );

    Ok(target.slot)
}

fn resolve_or_create_snapshot_dir(
    target: &EpochTarget,
    previous_snapshot_slot: Option<Slot>,
    ledger_snapshot_dir: &Path,
    context: &SnapshotBuildContext<'_>,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Some(snapshot_dir) = exact_snapshot_dir(ledger_snapshot_dir, target.slot) {
        info!(
            cli::db_analyser::REUSE_LEDGER_SNAPSHOT,
            epoch = %target.epoch,
            slot = %target.slot,
            snapshot = %relative_path(&snapshot_dir)?.display(),
        );
        return Ok(snapshot_dir);
    }

    let analyse_from = select_analyse_from_slot(ledger_snapshot_dir, target.slot, previous_snapshot_slot)?;

    info!(
        cli::db_analyser::RUN,
        epoch = %target.epoch,
        slot = %target.slot,
        analyse_from = @analyse_from.map(|s| s.to_string()),
    );

    if let Err(err) = run_db_analyser(
        context.db_analyser_binary,
        context.config_dir,
        context.cardano_node_db,
        target.slot,
        analyse_from,
        context.with_progress,
    ) {
        return Err(format!(
            "{err}; if immutable chunks are corrupt, delete {} and re-run to download fresh chunks from Mithril",
            context.cardano_node_db.join("immutable").display()
        )
        .into());
    }

    exact_snapshot_dir(ledger_snapshot_dir, target.slot)
        .ok_or_else(|| format!("db-analyser did not create snapshot directory for slot {}", target.slot).into())
}

fn write_packaged_blocks(
    target: &EpochTarget,
    immutable_dir: &Path,
    prepared_snapshot_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let blocks = packaged_blocks_for_target(immutable_dir, target)?;
    if blocks.is_empty() {
        return Ok(());
    }

    fs::write(prepared_snapshot_path.join(PACKAGED_BLOCKS_FILE_NAME), serde_json::to_vec_pretty(&blocks)?)?;

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

fn packaged_blocks_for_target(
    immutable_dir: &Path,
    target: &EpochTarget,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    packaged_blocks_from_iter(iter_immutable_blocks(immutable_dir)?, target, immutable_dir)
}

fn packaged_blocks_from_iter(
    blocks: impl IntoIterator<Item = Result<ImmutableBlock, Box<dyn std::error::Error>>>,
    target: &EpochTarget,
    immutable_dir: &Path,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    for block in blocks {
        let block = block?;
        if block.hash != target.hash {
            continue;
        }

        let header: BlockHeader = from_cbor(&block.header_cbor)
            .ok_or_else(|| format!("failed to decode target block header {} from immutable blocks", target.hash))?;
        if header.hash() != target.hash {
            return Err(
                format!("target block header hash mismatch: expected {}, got {}", target.hash, header.hash()).into()
            );
        }
        if header.slot() != target.slot {
            return Err(format!("target block slot mismatch: expected {}, got {}", target.slot, header.slot()).into());
        }
        if let Some(parent_point) = target.parent_point {
            let expected_parent = match parent_point {
                Point::Origin => None,
                Point::Specific(_, hash) => Some(hash),
            };
            if header.parent_hash() != expected_parent {
                return Err(format!(
                    "target block parent mismatch: expected {expected_parent:?}, got {:?}",
                    header.parent_hash()
                )
                .into());
            }
        }

        return Ok(vec![hex::encode(block.raw_block)]);
    }

    Err(format!(
        "target block {}.{} not found in immutable blocks under {}",
        target.slot,
        target.hash,
        immutable_dir.display()
    )
    .into())
}

#[cfg(test)]
mod tests {
    use std::{error::Error, fs, path::Path};

    use amaru_kernel::{BlockHeader, Epoch, HeaderHash, IsHeader, Point, Slot, make_header, to_cbor};
    use amaru_mithril::ImmutableBlock;
    use tempfile::TempDir;

    use super::{
        EpochTarget,
        archive::materialize_snapshot,
        bootstrap_target_epochs,
        db_analyser::{
            latest_snapshot_slot_at_or_before, parse_db_analyser_progress_line, parse_snapshot_slot_dir_name,
            select_analyse_from_slot,
        },
        packaged_blocks_from_iter,
    };

    fn immutable_block(header: &BlockHeader, raw_block: &[u8]) -> ImmutableBlock {
        ImmutableBlock { hash: header.hash(), header_cbor: to_cbor(header), raw_block: raw_block.to_vec() }
    }

    fn target_for(header: &BlockHeader, parent_point: Point) -> EpochTarget {
        EpochTarget {
            epoch: Epoch::from(1),
            slot: header.slot(),
            hash: header.hash(),
            parent_point: Some(parent_point),
        }
    }

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
    fn packages_only_the_exact_snapshot_block() {
        let parent_hash = HeaderHash::from([1; 32]);
        let parent_point = Point::Specific(Slot::from(41), parent_hash);
        let target_header = BlockHeader::from(make_header(2, 42, Some(parent_hash)));
        let child_header = BlockHeader::from(make_header(3, 43, Some(target_header.hash())));
        let target = target_for(&target_header, parent_point);
        let blocks = vec![
            Ok::<_, Box<dyn Error>>(immutable_block(&target_header, b"target")),
            Ok::<_, Box<dyn Error>>(immutable_block(&child_header, b"child")),
        ];

        assert_eq!(
            packaged_blocks_from_iter(blocks, &target, Path::new("immutable")).unwrap(),
            vec![hex::encode(b"target")]
        );
    }

    #[test]
    fn rejects_snapshot_block_with_inconsistent_metadata() {
        let parent_hash = HeaderHash::from([1; 32]);
        let header = BlockHeader::from(make_header(2, 42, Some(parent_hash)));
        let block = immutable_block(&header, b"target");

        let mut target = target_for(&header, Point::Specific(Slot::from(41), parent_hash));
        target.slot = Slot::from(43);
        assert!(
            packaged_blocks_from_iter(vec![Ok::<_, Box<dyn Error>>(block)], &target, Path::new("immutable")).is_err()
        );

        target.slot = header.slot();
        target.parent_point = Some(Point::Specific(Slot::from(41), HeaderHash::from([2; 32])));
        assert!(
            packaged_blocks_from_iter(
                vec![Ok::<_, Box<dyn Error>>(immutable_block(&header, b"target"))],
                &target,
                Path::new("immutable")
            )
            .is_err()
        );

        target.parent_point = Some(Point::Specific(Slot::from(41), parent_hash));
        let mismatched_header = BlockHeader::from(make_header(3, 42, Some(parent_hash)));
        let mismatched = ImmutableBlock { hash: target.hash, ..immutable_block(&mismatched_header, b"other") };
        assert!(
            packaged_blocks_from_iter(vec![Ok::<_, Box<dyn Error>>(mismatched)], &target, Path::new("immutable"))
                .is_err()
        );
    }
}
