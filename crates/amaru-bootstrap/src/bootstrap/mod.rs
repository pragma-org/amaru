// Copyright 2025 PRAGMA
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
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Read},
    path::{Component, Path, PathBuf},
    time::Duration,
};

use amaru_kernel::{
    Credential, Epoch, GlobalParameters, Hash, Header, HeaderHash, IsHeader, NetworkName, NetworkPoint, Nonce, Point,
    RawBlock, Slot, extract_block_header_cbor, from_cbor, num::CheckedSub, utils::string::display_collection,
};
use amaru_ledger::store::{EpochTransitionProgress, ReadStore, Store, TransactionalContext};
use amaru_observability::{error, info};
use amaru_ouroboros::{BaseReadChainStore, ChainStore, Nonces, OpcertSequenceNumbers, WriteChainStore};
use amaru_stores::rocksdb::{ReadOnlyRocksDB, RocksDB, RocksDbConfig, consensus::RocksDBStore};
use anyhow::anyhow;
use pallas_network::{facades::PeerClient, miniprotocols::chainsync::NextResponse};
use serde::{Deserialize, Serialize};
use tar::Archive;
use tokio::{
    fs as async_fs,
    time::{Instant, timeout},
};
use zstd::Decoder as ZstdDecoder;

mod chain_sync_client;
use chain_sync_client::{ChainSyncClient, from_pallas_point, from_pallas_tip};

use crate::{
    aws::{AnonymousS3Client, S3Config},
    cardano_node::tvar::{import_state_from_tvar, import_utxo_from_tvar},
    progress::BootstrapProgressFactory,
};

/// S3-backed snapshot descriptor used during bootstrap.
#[derive(Debug, Clone)]
struct Snapshot {
    epoch: Epoch,
    point: String,
    /// Full S3 object key: `<network>/<point>.tar.zst`.
    key: String,
}

impl Snapshot {
    pub const ARCHIVE_SUFFIX: &str = ".tar.zst";
}

#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    #[error("Can not create snapshots directory {0}: {1}")]
    CreateSnapshotsDir(PathBuf, io::Error),

    #[error("Unable to store snapshots on disk: {0}")]
    Io(#[from] io::Error),

    #[error("Failed to download snapshot {0}: {1}")]
    DownloadError(String, String),

    #[error("Missing bootstrap snapshot {0}")]
    MissingSnapshot(PathBuf),

    #[error("Invalid snapshot archive {0}: {1}")]
    InvalidSnapshotArchive(PathBuf, String),

    #[error("No bootstrap snapshots found in S3 bucket for this network")]
    NoBootstrapSnapshots,

    #[error(
        "requested {target_epoch}, but {} available for bootstrap on this network.",
        format_available_epochs(available_epochs)
    )]
    SnapshotSelectionRequestedEpoch { target_epoch: Epoch, available_epochs: Vec<Epoch> },

    #[error(
        "bootstrap needs the latest 3 consecutive snapshot epochs ending at {latest_epoch}, but S3 bucket {} available. Required epochs: [{}]",
        format_available_epochs(available_epochs),
        display_collection(required_epochs)
    )]
    SnapshotSelectionLatestEpoch { latest_epoch: Epoch, required_epochs: [Epoch; 3], available_epochs: Vec<Epoch> },
}

pub const BOOTSTRAP_HEADERS_PER_POINT: usize = 2;
const PACKAGED_BLOCKS_FILE_NAME: &str = "bootstrap.blocks.json";
const SNAPSHOT_STATE_FILE_NAME: &str = "state";
const SNAPSHOT_UTXO_FILE_NAME: &str = "tables/tvar";

fn snapshot_archive_path(snapshots_dir: &Path, snapshot: &Snapshot) -> PathBuf {
    snapshots_dir.join(format!("{}{}", snapshot.point, Snapshot::ARCHIVE_SUFFIX))
}

fn resolve_snapshot_path(snapshots_dir: &Path, snapshot: &Snapshot) -> Option<PathBuf> {
    let archive = snapshot_archive_path(snapshots_dir, snapshot);
    is_snapshot_archive_file(&archive).then_some(archive)
}

fn snapshot_hash(snapshot: &Snapshot) -> anyhow::Result<HeaderHash> {
    match NetworkPoint::try_from(snapshot.point.as_str()).map_err(anyhow::Error::msg)? {
        NetworkPoint::Specific(_, hash) => Ok(hash),
        NetworkPoint::Origin => Err(anyhow!("bootstrap snapshots must not use origin")),
    }
}

/// List S3 objects under `<network>/`, derive epoch from slot via era history, return `Vec<Snapshot>`.
async fn bootstrap_snapshots(
    network: NetworkName,
    s3: &AnonymousS3Client,
    snapshots_dir: &Path,
) -> anyhow::Result<Vec<Snapshot>> {
    let era_history = network
        .as_era_history()
        .ok_or_else(|| anyhow!("no era history available for network {network}; S3 bootstrap is only supported for mainnet, preprod, and preview"))?;

    let s3_snapshots = s3.list_snapshots(network).await?;

    let mut snapshots = Vec::with_capacity(s3_snapshots.len());

    let try_push = |snapshots: &mut Vec<Snapshot>, point: String, key: String| -> anyhow::Result<()> {
        let slot = parse_slot_from_point(&point)?;
        let epoch = era_history.slot_to_epoch_unchecked_horizon(Slot::from(slot))?;
        snapshots.push(Snapshot { epoch, point, key });
        Ok(())
    };

    for s3_snap in s3_snapshots {
        try_push(&mut snapshots, s3_snap.point.to_string(), s3_snap.key.to_string())?;
    }

    match snapshots_dir.read_dir() {
        Ok(entries) => {
            for dir_entry in entries {
                let path = dir_entry?.path();
                let filename = path.file_name().unwrap_or_default().to_str().unwrap_or_default();
                if let Some(prefix) = filename.strip_suffix(Snapshot::ARCHIVE_SUFFIX) {
                    try_push(
                        &mut snapshots,
                        prefix.to_string(),
                        format!("{network}/{prefix}{}", Snapshot::ARCHIVE_SUFFIX),
                    )?;
                }
            }
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            // If there is no directory already created, we should not fail
        }
        Err(err) => return Err(err.into()),
    }

    snapshots.sort_unstable_by_key(|snapshot| snapshot.epoch);
    snapshots.dedup_by_key(|snapshot| snapshot.epoch);

    Ok(snapshots)
}

/// Parse the slot number from a `<slot>.<hash>` point string.
fn parse_slot_from_point(point: &str) -> anyhow::Result<u64> {
    point
        .split('.')
        .next()
        .and_then(|s| s.parse::<u64>().ok())
        .ok_or_else(|| anyhow!("invalid snapshot point format: {point}"))
}

fn format_available_epochs(epochs: &[Epoch]) -> String {
    if epochs.is_empty() {
        "none are".to_string()
    } else {
        format!("only {} {}", display_collection(epochs), if epochs.len() > 1 { "are" } else { "is" },)
    }
}

fn select_bootstrap_snapshots(snapshots: &[Snapshot], target_epoch: Option<Epoch>) -> anyhow::Result<[&Snapshot; 3]> {
    let snapshots_by_epoch: BTreeMap<Epoch, &Snapshot> = snapshots.iter().map(|s| (s.epoch, s)).collect();
    let latest_epoch = snapshots_by_epoch.keys().next_back().copied().ok_or(BootstrapError::NoBootstrapSnapshots)?;
    let first_epoch = target_epoch
        .map(|target| {
            target
                .checked_sub(Epoch::THREE)
                .ok_or_else(|| anyhow!("target epoch is too young; Amaru needs at least 3 past epochs to bootstrap."))
        })
        .transpose()?
        .unwrap_or_else(|| latest_epoch.saturating_sub(2));
    let required_epochs = [first_epoch, first_epoch + 1, first_epoch + 2];

    match required_epochs.map(|epoch| snapshots_by_epoch.get(&epoch).copied()) {
        [Some(first_snapshot), Some(second_snapshot), Some(third_snapshot)] => {
            Ok([first_snapshot, second_snapshot, third_snapshot])
        }
        _ => {
            let mut available_epochs = Vec::new();

            let mut count = 0;
            let mut prev_epoch = Epoch::from(u64::MAX - 1);
            for epoch in snapshots_by_epoch.keys().copied() {
                if epoch == prev_epoch + 1 {
                    prev_epoch = epoch;
                    count += 1;
                } else {
                    prev_epoch = epoch;
                    count = 1;
                    continue;
                }

                if count >= 3 {
                    available_epochs.push(epoch + 1)
                }
            }

            match target_epoch {
                Some(target_epoch) => {
                    Err(BootstrapError::SnapshotSelectionRequestedEpoch { target_epoch, available_epochs }.into())
                }
                None => Err(BootstrapError::SnapshotSelectionLatestEpoch {
                    latest_epoch,
                    required_epochs,
                    available_epochs,
                }
                .into()),
            }
        }
    }
}

pub async fn fetch_headers_from_points(
    peer_address: &str,
    network: NetworkName,
    points: &[NetworkPoint],
    headers_per_point: usize,
) -> anyhow::Result<Vec<Vec<u8>>> {
    if points.is_empty() || headers_per_point == 0 {
        return Ok(Vec::new());
    }

    let mut headers = Vec::with_capacity(points.len().saturating_mul(headers_per_point));
    for point in points {
        headers.extend(fetch_headers_from_point(peer_address, network, *point, headers_per_point).await?);
    }

    Ok(headers)
}

async fn fetch_headers_from_point(
    peer_address: &str,
    network: NetworkName,
    point: NetworkPoint,
    headers_per_point: usize,
) -> anyhow::Result<Vec<Vec<u8>>> {
    let peer_client =
        PeerClient::connect(peer_address, network.to_network_magic().as_u64()).await.inspect_err(|err| {
            error!(bootstrap::peer::FAILED_TO_CONNECT, peer = peer_address, reason = err.to_string());
        })?;
    let mut client = ChainSyncClient::new(peer_address, peer_client.chainsync, vec![point]);
    let intersection = client.find_intersection().await?;

    info!(bootstrap::headers::FETCH, requested_point = point, intersection, headers_per_point);

    let mut headers = Vec::with_capacity(headers_per_point);
    while headers.len() < headers_per_point {
        let next = if client.has_agency() {
            client.request_next().await?
        } else {
            match timeout(Duration::from_secs(1), client.await_next()).await {
                Ok(next) => next?,
                Err(_) => continue,
            }
        };

        match next {
            NextResponse::RollForward(content, tip) => {
                let block_header: Header =
                    from_cbor(&content.cbor).ok_or_else(|| anyhow!("failed to decode fetched block header"))?;
                let slot = u64::from(block_header.slot());
                headers.push(content.cbor);

                if headers.len() >= headers_per_point || slot == tip.0.slot_or_default() {
                    break;
                }
            }
            NextResponse::RollBackward(point, tip) => {
                let point = from_pallas_point(&point);
                let tip = from_pallas_tip(&tip);
                info!(bootstrap::fetch::ROLLBACK, point, tip);
            }
            NextResponse::Await => continue,
        }
    }

    Ok(headers)
}

fn should_download_snapshot(snapshots_dir: &Path, snapshot: &Snapshot) -> bool {
    resolve_snapshot_path(snapshots_dir, snapshot).is_none()
}

async fn download_snapshots(
    snapshots: &[&Snapshot],
    snapshots_dir: &Path,
    s3: &AnonymousS3Client,
) -> Result<(), BootstrapError> {
    async_fs::create_dir_all(snapshots_dir)
        .await
        .map_err(|err| BootstrapError::CreateSnapshotsDir(snapshots_dir.to_path_buf(), err))?;

    for snapshot in snapshots {
        let archive_path = snapshot_archive_path(snapshots_dir, snapshot);

        if !should_download_snapshot(snapshots_dir, snapshot) {
            validate_snapshot_archive(&archive_path)
                .map_err(|err| BootstrapError::InvalidSnapshotArchive(archive_path.clone(), err.to_string()))?;
            let snapshot_path = resolve_snapshot_path(snapshots_dir, snapshot).unwrap_or_else(|| archive_path.clone());
            info!(bootstrap::snapshot::SKIP_DOWNLOAD, snapshot = snapshot_path.display().to_string());
            continue;
        }

        if archive_path.exists() {
            return Err(BootstrapError::InvalidSnapshotArchive(
                archive_path.clone(),
                format!("snapshot archive path exists but is not a regular `{}` file", Snapshot::ARCHIVE_SUFFIX),
            ));
        }

        let partial_archive_path = snapshots_dir.join(format!(".{}.download.partial", snapshot.point));

        info!(bootstrap::snapshot::DOWNLOAD, epoch = snapshot.epoch, point = snapshot.point);

        s3.download_object(&snapshot.key, &partial_archive_path)
            .await
            .map_err(|e| BootstrapError::DownloadError(snapshot.key.clone(), e.to_string()))?;

        validate_snapshot_archive(&partial_archive_path)
            .map_err(|err| BootstrapError::InvalidSnapshotArchive(partial_archive_path.clone(), err.to_string()))?;

        async_fs::rename(partial_archive_path, archive_path).await?;
    }

    Ok(())
}

fn snapshot_archive_root(archive_path: &Path) -> anyhow::Result<PathBuf> {
    let archive_file = fs::File::open(archive_path)?;
    let mut archive = Archive::new(ZstdDecoder::new(archive_file)?);
    let mut state_root = None;
    let mut utxo_root = None;

    for entry in archive.entries()? {
        let entry = entry?;
        let path = entry.path()?;
        state_root = state_root.or_else(|| snapshot_archive_entry_root(&path, Path::new(SNAPSHOT_STATE_FILE_NAME)));
        utxo_root = utxo_root.or_else(|| snapshot_archive_entry_root(&path, Path::new(SNAPSHOT_UTXO_FILE_NAME)));
    }

    match (state_root, utxo_root) {
        (Some(state_root), Some(utxo_root)) if state_root == utxo_root => Ok(state_root),
        (Some(_), Some(_)) => Err(anyhow!("snapshot archive state and tables/tvar must share the same root directory")),
        _ => Err(anyhow!("archive must contain {SNAPSHOT_STATE_FILE_NAME} and {SNAPSHOT_UTXO_FILE_NAME}")),
    }
}

fn validate_snapshot_archive(archive_path: &Path) -> anyhow::Result<()> {
    snapshot_archive_root(archive_path).map(|_| ())
}

pub fn validate_publishable_snapshot_archive(archive_path: &Path, expected_point: &str) -> anyhow::Result<()> {
    let root = snapshot_archive_root(archive_path)?;
    if root != Path::new(expected_point) {
        return Err(anyhow!("snapshot archive root is {}, expected {expected_point}", root.display()));
    }

    load_packaged_block_from_snapshot(archive_path, expected_point).map(|_| ())
}

fn read_snapshot_archive_entry(archive_path: &Path, expected: &Path) -> anyhow::Result<Vec<u8>> {
    let archive_file = fs::File::open(archive_path)?;
    let mut archive = Archive::new(ZstdDecoder::new(archive_file)?);

    for entry in archive.entries()? {
        let mut entry = entry?;
        if snapshot_archive_entry_matches(&entry.path()?, expected) {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            return Ok(bytes);
        }
    }

    Err(anyhow!("snapshot archive {} does not contain {}", archive_path.display(), expected.display()))
}

fn is_snapshot_archive_file(path: &Path) -> bool {
    path.is_file() && has_snapshot_archive_extension(path)
}

fn has_snapshot_archive_extension(path: &Path) -> bool {
    path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.ends_with(Snapshot::ARCHIVE_SUFFIX))
}

fn snapshot_archive_entry_matches(path: &Path, expected: &Path) -> bool {
    snapshot_archive_entry_root(path, expected).is_some()
}

fn snapshot_archive_entry_root(path: &Path, expected: &Path) -> Option<PathBuf> {
    fn relative_components(path: &Path) -> Option<Vec<&std::ffi::OsStr>> {
        path.components().try_fold(Vec::new(), |mut components, component| match component {
            Component::Normal(segment) => {
                components.push(segment);
                Some(components)
            }
            Component::CurDir => Some(components),
            Component::Prefix(_) | Component::RootDir | Component::ParentDir => None,
        })
    }

    let path_components = relative_components(path)?;
    let expected_components = relative_components(expected)?;

    if path_components == expected_components {
        Some(PathBuf::new())
    } else if path_components.len() == expected_components.len() + 1 && path_components[1..] == expected_components {
        Some(PathBuf::from(path_components[0]))
    } else {
        None
    }
}

/// Set the internal dbs in such a state that amaru can run
pub async fn bootstrap(
    network: NetworkName,
    global_parameters: &GlobalParameters,
    ledger_dir: PathBuf,
    chain_dir: PathBuf,
    snapshots_dir: PathBuf,
    target_epoch: Option<Epoch>,
    s3_config: S3Config,
) -> anyhow::Result<()> {
    let started_at = Instant::now();
    let s3 = AnonymousS3Client::new(s3_config);
    let snapshots = bootstrap_snapshots(network, &s3, &snapshots_dir).await?;
    let [first_snapshot, second_snapshot, third_snapshot] = select_bootstrap_snapshots(&snapshots, target_epoch)?;

    download_snapshots(&[first_snapshot, second_snapshot, third_snapshot], &snapshots_dir, &s3).await?;

    let first_snapshot_path = resolve_snapshot_path(&snapshots_dir, first_snapshot)
        .ok_or_else(|| BootstrapError::MissingSnapshot(snapshot_archive_path(&snapshots_dir, first_snapshot)))?;
    let second_snapshot_path = resolve_snapshot_path(&snapshots_dir, second_snapshot)
        .ok_or_else(|| BootstrapError::MissingSnapshot(snapshot_archive_path(&snapshots_dir, second_snapshot)))?;
    let third_snapshot_path = resolve_snapshot_path(&snapshots_dir, third_snapshot)
        .ok_or_else(|| BootstrapError::MissingSnapshot(snapshot_archive_path(&snapshots_dir, third_snapshot)))?;

    import_snapshot(network, global_parameters, &first_snapshot_path, &ledger_dir).await?;

    // Extract nonces for the second snapshot tip as well: packaged bootstrap headers must enter
    // the chain store already carrying nonces so that "nonces present ⇔ header validated" holds
    // with no bootstrap exception.
    let imported_second_snapshot = import_snapshot_with_optional_nonces(
        network,
        global_parameters,
        &second_snapshot_path,
        &ledger_dir,
        Some(snapshot_hash(first_snapshot)?),
    )
    .await?;

    let imported_third_snapshot = import_snapshot_with_optional_nonces(
        network,
        global_parameters,
        &third_snapshot_path,
        &ledger_dir,
        Some(snapshot_hash(second_snapshot)?),
    )
    .await?;

    let chain_db = RocksDBStore::create(RocksDbConfig::new(chain_dir.clone()))?;
    let second_chain_state = imported_second_snapshot
        .chain_state
        .ok_or_else(|| anyhow!("bootstrap import must produce the chain state for the second snapshot"))?;
    let third_chain_state = imported_third_snapshot
        .chain_state
        .ok_or_else(|| anyhow!("bootstrap import must produce the chain state for the latest snapshot"))?;
    // Nonces for both packaged tips are stored before the headers so
    // `import_packaged_blocks` can attach each header via `store_validated_header`.
    store_chain_state(imported_second_snapshot.epoch, &chain_db, second_chain_state)?;
    store_chain_state(imported_third_snapshot.epoch, &chain_db, third_chain_state)?;
    let blocks = load_packaged_blocks_for_bootstrap(
        &second_snapshot_path,
        second_snapshot,
        &third_snapshot_path,
        third_snapshot,
    )?;
    import_packaged_blocks(&chain_db, blocks).await?;

    info!(
        bootstrap::COMPLETE,
        duration_seconds = started_at.elapsed().as_secs_f64(),
        epoch = imported_third_snapshot.epoch,
        point = third_snapshot.point.clone(),
    );

    Ok(())
}

pub async fn import_packaged_blocks(db: &RocksDBStore, blocks: Vec<Vec<u8>>) -> anyhow::Result<()> {
    for block in blocks {
        let header_cbor = extract_block_header_cbor(&block)?;
        let block_header: Header =
            from_cbor(header_cbor).ok_or_else(|| anyhow!("failed to decode packaged bootstrap block header"))?;
        let hash = block_header.hash();

        info!(bootstrap::header::IMPORT, header = hash);

        // Packaged bootstrap headers are trusted as fully validated; nonces must already have
        // been imported for each tip so the durable "nonces present ⇔ header validated" invariant
        // holds with no special case for bootstrap.
        let nonces = db.get_nonces(&hash).ok_or_else(|| {
            anyhow!("bootstrap packaged header {hash} is missing nonces; refuse to store incomplete tip")
        })?;
        db.store_validated_header(&block_header, &nonces)?;
        db.store_block(&hash, &RawBlock::from(block.as_slice()))?;
    }

    Ok(())
}

fn load_packaged_blocks_for_bootstrap(
    second_snapshot_path: &Path,
    second_snapshot: &Snapshot,
    third_snapshot_path: &Path,
    third_snapshot: &Snapshot,
) -> anyhow::Result<Vec<Vec<u8>>> {
    Ok(vec![
        load_packaged_block_from_snapshot(second_snapshot_path, &second_snapshot.point)?,
        load_packaged_block_from_snapshot(third_snapshot_path, &third_snapshot.point)?,
    ])
}

fn load_packaged_block_from_snapshot(snapshot_path: &Path, expected_point: &str) -> anyhow::Result<Vec<u8>> {
    if !is_snapshot_archive_file(snapshot_path) {
        return Err(anyhow!("snapshot does not contain packaged bootstrap blocks: {}", snapshot_path.display()));
    }

    let bytes = read_snapshot_archive_entry(snapshot_path, Path::new(PACKAGED_BLOCKS_FILE_NAME))?;
    let hex_blocks: Vec<String> = serde_json::from_slice(&bytes)?;
    let hex_block = require_single_packaged_block(hex_blocks, snapshot_path)?;

    let block = hex::decode(hex_block)?;
    let header_cbor = extract_block_header_cbor(&block)?;
    let header: Header =
        from_cbor(header_cbor).ok_or_else(|| anyhow!("failed to decode packaged bootstrap block header"))?;
    let expected_point = NetworkPoint::try_from(expected_point).map_err(anyhow::Error::msg)?;
    if header.point().to_network_point() != expected_point {
        return Err(anyhow!(
            "packaged bootstrap block is {}, expected {expected_point}",
            header.point().to_network_point()
        ));
    }

    Ok(block)
}

fn require_single_packaged_block(mut hex_blocks: Vec<String>, snapshot_path: &Path) -> anyhow::Result<String> {
    if hex_blocks.len() != 1 {
        return Err(anyhow!(
            "packaged bootstrap blocks at {} contain {} blocks; expected exactly 1",
            snapshot_path.display(),
            hex_blocks.len()
        ));
    }

    Ok(hex_blocks.remove(0))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainState {
    pub initial_nonces: InitialNonces,
    pub opcert_sequence_numbers: OpcertSequenceNumbers,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitialNonces {
    pub at: Point,
    pub active: Nonce,
    pub evolving: Nonce,
    pub candidate: Nonce,
    pub tail: HeaderHash,
}

pub fn store_chain_state(epoch: Epoch, db: &dyn ChainStore, chain_state: ChainState) -> anyhow::Result<()> {
    let initial_nonces = chain_state.initial_nonces;
    let header_hash = Hash::from(&initial_nonces.at);

    info!(bootstrap::nonces::IMPORT, point = initial_nonces.at);

    let nonces = Nonces {
        epoch,
        active: initial_nonces.active,
        evolving: initial_nonces.evolving,
        candidate: initial_nonces.candidate,
        tail: initial_nonces.tail,
    };

    db.put_nonces(&header_hash, &nonces)?;

    info!(bootstrap::opcert_sequence_numbers::IMPORT, point = initial_nonces.at);
    db.put_opcert_seed(&chain_state.opcert_sequence_numbers, &initial_nonces.at)?;

    Ok(())
}

pub async fn import_headers(db: &RocksDBStore, headers: Vec<Vec<u8>>) -> anyhow::Result<()> {
    for header in headers {
        let block_header: Header =
            from_cbor(&header).ok_or_else(|| anyhow!("failed to decode packaged bootstrap header"))?;
        let hash = block_header.hash();

        info!(bootstrap::header::IMPORT, header = hash);

        db.store_header(&block_header)?;
    }

    Ok(())
}

pub async fn import_snapshots_from_directory(
    network: NetworkName,
    global_parameters: &GlobalParameters,
    ledger_dir: &Path,
    snapshot_dir: &Path,
) -> anyhow::Result<()> {
    let mut snapshots = fs::read_dir(snapshot_dir)?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.is_file() && has_snapshot_archive_extension(path))
        .collect::<Vec<_>>();

    sort_snapshots_by_slot(&mut snapshots);

    import_snapshots(network, global_parameters, &snapshots, ledger_dir).await
}

fn sort_snapshots_by_slot(snapshots: &mut [PathBuf]) {
    // Sort by parsed slot number from the leading `<slot>.<hash>` path component.
    snapshots.sort_by_key(|path| {
        path.file_name()
            .and_then(|s| s.to_str())
            .and_then(|s| s.split('.').next())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(u64::MAX)
    });
}

pub async fn import_snapshots(
    network: NetworkName,
    global_parameters: &GlobalParameters,
    snapshots: &[PathBuf],
    ledger_dir: &Path,
) -> anyhow::Result<()> {
    info!(bootstrap::snapshots::IMPORT, count = snapshots.len());
    for snapshot in snapshots {
        import_snapshot(network, global_parameters, snapshot, ledger_dir).await?;
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("expected a snapshot archive in `{}` format: {}", Snapshot::ARCHIVE_SUFFIX, .0.display())]
    UnsupportedSnapshotPath(PathBuf),
}

struct ImportedSnapshot {
    epoch: Epoch,
    chain_state: Option<ChainState>,
}

async fn import_snapshot(
    network: NetworkName,
    global_parameters: &GlobalParameters,
    snapshot: &Path,
    ledger_dir: &Path,
) -> anyhow::Result<()> {
    import_snapshot_with_optional_nonces(network, global_parameters, snapshot, ledger_dir, None).await?;
    Ok(())
}

async fn import_snapshot_with_optional_nonces(
    network: NetworkName,
    global_parameters: &GlobalParameters,
    snapshot: &Path,
    ledger_dir: &Path,
    nonce_tail: Option<HeaderHash>,
) -> anyhow::Result<ImportedSnapshot> {
    if snapshot.is_file() && has_snapshot_archive_extension(snapshot) {
        return import_node_snapshot_archive(network, global_parameters, snapshot, ledger_dir, nonce_tail).await;
    }

    Err(ImportError::UnsupportedSnapshotPath(snapshot.to_path_buf()).into())
}

async fn import_node_snapshot_archive(
    network: NetworkName,
    global_parameters: &GlobalParameters,
    snapshot_archive: &Path,
    ledger_dir: &Path,
    nonce_tail: Option<HeaderHash>,
) -> anyhow::Result<ImportedSnapshot> {
    info!(bootstrap::snapshot::IMPORT_ARCHIVE, path = snapshot_archive.display().to_string());
    import_node_snapshot_source(network, global_parameters, snapshot_archive, ledger_dir, nonce_tail).await
}

#[expect(clippy::unwrap_used)]
async fn import_node_snapshot_source(
    network: NetworkName,
    global_parameters: &GlobalParameters,
    archive_path: &Path,
    ledger_dir: &Path,
    nonce_tail: Option<HeaderHash>,
) -> anyhow::Result<ImportedSnapshot> {
    fs::create_dir_all(ledger_dir)?;

    let previous_accounts = if fs::exists(ledger_dir.join("live"))? {
        let live = ReadOnlyRocksDB::new(&RocksDbConfig::new(ledger_dir.to_path_buf()))?;
        let previous_accounts = live
            .iter_accounts()?
            .map(|(credential, _)| credential)
            .chain(live.iter_recently_unregistered_accounts()?)
            .collect();

        fs::remove_dir_all(ledger_dir.join("live"))?;

        previous_accounts
    } else {
        BTreeSet::new()
    };

    let db = RocksDB::empty(&RocksDbConfig::new(ledger_dir.to_path_buf()))?;
    let global_parameters = global_parameters.clone();
    let archive_path = archive_path.to_path_buf();
    let builder = std::thread::Builder::new().stack_size(10_000_000);

    let (db, epoch, chain_state) = builder
        .spawn(move || {
            import_node_snapshot_archive_data(
                &archive_path,
                &db,
                network,
                &global_parameters,
                nonce_tail,
                previous_accounts,
            )
            .map(|(epoch, _point, chain_state)| (db, epoch, chain_state))
        })
        .unwrap()
        .join()
        .unwrap()?;

    db.next_snapshot(epoch)?;

    db.with_transaction(|batch| batch.try_epoch_transition(None, Some(EpochTransitionProgress::SnapshotTaken)))?;

    Ok(ImportedSnapshot { epoch, chain_state })
}

fn import_node_snapshot_archive_data(
    archive_path: &Path,
    db: &RocksDB,
    network: NetworkName,
    global_parameters: &GlobalParameters,
    nonce_tail: Option<HeaderHash>,
    previous_accounts: BTreeSet<Credential>,
) -> anyhow::Result<(Epoch, Point, Option<ChainState>)> {
    let archive_file = fs::File::open(archive_path)?;
    let mut archive = Archive::new(ZstdDecoder::new(archive_file)?);

    let mut imported_state = None;
    let mut previous_accounts: Option<BTreeSet<Credential>> = Some(previous_accounts);

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();

        if snapshot_archive_entry_matches(&path, Path::new(SNAPSHOT_STATE_FILE_NAME)) {
            imported_state = Some(import_state_from_tvar(
                db,
                &mut entry,
                network,
                global_parameters,
                nonce_tail,
                previous_accounts.take().ok_or_else(|| {
                    anyhow!("multiple {SNAPSHOT_STATE_FILE_NAME} in archive {}?", archive_path.display())
                })?,
                &BootstrapProgressFactory,
            )?);
        } else if snapshot_archive_entry_matches(&path, Path::new(SNAPSHOT_UTXO_FILE_NAME)) {
            let (epoch, point, era_history, chain_state) = imported_state.take().ok_or_else(|| {
                anyhow!(
                    "snapshot archive {} contains {SNAPSHOT_UTXO_FILE_NAME} before {SNAPSHOT_STATE_FILE_NAME}",
                    archive_path.display()
                )
            })?;

            import_utxo_from_tvar(&mut entry, db, &BootstrapProgressFactory, &point, &era_history, network)?;

            return Ok((epoch, point, chain_state));
        }
    }

    Err(anyhow!("snapshot archive {} does not contain {SNAPSHOT_UTXO_FILE_NAME}", archive_path.display()))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::Cursor,
        path::{Path, PathBuf},
        time::Duration,
    };

    use amaru_kernel::{BlockHeight, Epoch, EraBound, EraHistory, EraName, EraParams, EraSummary, HeaderHash, Slot};
    use tar::{Builder, Header};
    use tempfile::tempdir;

    use super::{
        Snapshot, read_snapshot_archive_entry, select_bootstrap_snapshots, should_download_snapshot,
        snapshot_archive_entry_matches, sort_snapshots_by_slot, validate_publishable_snapshot_archive,
        validate_snapshot_archive,
    };
    use crate::cardano_node::ParsedStateSnapshot;

    fn test_snapshot(epoch: u64, point: &str, network: &str) -> Snapshot {
        Snapshot {
            epoch: Epoch::from(epoch),
            point: point.to_string(),
            key: format!("{network}/{point}{}", Snapshot::ARCHIVE_SUFFIX),
        }
    }

    fn snapshot_epoch(parsed_snapshot: &ParsedStateSnapshot) -> anyhow::Result<Epoch> {
        Ok(parsed_snapshot.era_history.slot_to_epoch_unchecked_horizon(parsed_snapshot.slot.into())?)
    }

    fn write_snapshot_archive(path: &Path, entries: &[(&str, &[u8])]) {
        let file = fs::File::create(path).unwrap();
        let encoder = zstd::Encoder::new(file, 0).unwrap();
        let mut archive = Builder::new(encoder);

        for (entry_path, bytes) in entries {
            let mut header = Header::new_gnu();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_mode(0o644);
            header.set_size(bytes.len() as u64);
            header.set_cksum();
            archive.append_data(&mut header, entry_path, Cursor::new(bytes)).unwrap();
        }

        archive.into_inner().unwrap().finish().unwrap();
    }

    #[test]
    fn sort_snapshot_paths_by_slot_number() {
        let mut paths = [
            PathBuf::from("172786.932b9688167139cf4792e97ae4771b6dc762ad25752908cce7b24c2917847516"),
            PathBuf::from("259174.a07da7616822a1ccb4811e907b1f3a3c5274365908a241f4d5ffab2a69eb8802"),
            PathBuf::from("86392.1d38de4ffae6090c24151578d331b1021adb8f37d158011616db4d47d1704968"),
        ];

        sort_snapshots_by_slot(&mut paths);

        assert_eq!(PathBuf::from("86392.1d38de4ffae6090c24151578d331b1021adb8f37d158011616db4d47d1704968"), paths[0]);
    }

    #[test]
    fn snapshot_epoch_uses_snapshot_era_history() {
        let era_history = EraHistory::new(
            &[EraSummary {
                start: EraBound { time: Duration::from_secs(0), slot: Slot::from(0_u64), epoch: Epoch::from(10_u64) },
                end: None,
                params: EraParams {
                    epoch_size_slots: 5,
                    slot_length: Duration::from_secs(1),
                    era_name: EraName::Conway,
                },
            }],
            Slot::from(0_u64),
        );
        let parsed_snapshot = ParsedStateSnapshot {
            slot: 12,
            hash: HeaderHash::from([0_u8; 32]),
            block_height: BlockHeight::from(12),
            era_history,
            ledger_data_begin: 0,
            ledger_data_end: 0,
        };

        assert_eq!(snapshot_epoch(&parsed_snapshot).unwrap(), Epoch::from(12_u64));
    }

    #[test]
    fn should_not_download_existing_snapshot_archive() {
        let temp_dir = tempdir().unwrap();
        let snapshot = test_snapshot(163, "69206375.hash", "preprod");
        let archive_path = temp_dir.path().join("69206375.hash.tar.zst");
        write_snapshot_archive(
            &archive_path,
            &[("69206375.hash/state", b"state"), ("69206375.hash/tables/tvar", b"utxo")],
        );

        validate_snapshot_archive(&archive_path).unwrap();
        assert_eq!(read_snapshot_archive_entry(&archive_path, Path::new("state")).unwrap(), b"state");
        assert!(!should_download_snapshot(temp_dir.path(), &snapshot));
    }

    #[test]
    fn rejects_snapshot_archive_without_tvar() {
        let temp_dir = tempdir().unwrap();
        let archive_path = temp_dir.path().join("69206375.hash.tar.zst");
        write_snapshot_archive(&archive_path, &[("69206375.hash/state", b"state")]);

        assert!(validate_snapshot_archive(&archive_path).is_err());
    }

    #[test]
    fn rejects_snapshot_archive_with_multiple_roots() {
        let temp_dir = tempdir().unwrap();
        let archive_path = temp_dir.path().join("69206375.hash.tar.zst");
        write_snapshot_archive(&archive_path, &[("first/state", b"state"), ("second/tables/tvar", b"utxo")]);

        assert!(validate_snapshot_archive(&archive_path).is_err());
    }

    #[test]
    fn publish_validation_rejects_archive_root_that_does_not_match_point() {
        let temp_dir = tempdir().unwrap();
        let archive_path = temp_dir.path().join("69206375.hash.tar.zst");
        write_snapshot_archive(
            &archive_path,
            &[("wrong/state", b"state"), ("wrong/tables/tvar", b"utxo"), ("wrong/bootstrap.blocks.json", br#"["00"]"#)],
        );

        assert!(validate_publishable_snapshot_archive(&archive_path, "69206375.hash").is_err());
    }

    #[test]
    fn snapshot_archive_entries_allow_one_root_directory_only() {
        assert!(snapshot_archive_entry_matches(Path::new("snapshot/tables/tvar"), Path::new("tables/tvar")));
        assert!(snapshot_archive_entry_matches(Path::new("tables/tvar"), Path::new("tables/tvar")));
        assert!(!snapshot_archive_entry_matches(Path::new("outer/snapshot/tables/tvar"), Path::new("tables/tvar")));
        assert!(!snapshot_archive_entry_matches(Path::new("../tables/tvar"), Path::new("tables/tvar")));
    }

    #[test]
    fn select_bootstrap_snapshots_defaults_to_latest_epoch_window() {
        let snapshots = vec![
            test_snapshot(165, "70070379.hash3", "preprod"),
            test_snapshot(163, "69206375.hash1", "preprod"),
            test_snapshot(164, "69638382.hash2", "preprod"),
        ];

        let [first_snapshot, second_snapshot, third_snapshot] = select_bootstrap_snapshots(&snapshots, None).unwrap();

        assert_eq!(first_snapshot.epoch, Epoch::from(163_u64));
        assert_eq!(second_snapshot.epoch, Epoch::from(164_u64));
        assert_eq!(third_snapshot.epoch, Epoch::from(165_u64));
    }

    #[test]
    fn select_bootstrap_snapshots_honors_requested_start_epoch() {
        let snapshots = vec![
            test_snapshot(163, "69206375.hash1", "preprod"),
            test_snapshot(164, "69638382.hash2", "preprod"),
            test_snapshot(165, "70070379.hash3", "preprod"),
            test_snapshot(166, "70502379.hash4", "preprod"),
        ];

        let [first_snapshot, second_snapshot, third_snapshot] =
            select_bootstrap_snapshots(&snapshots, Some(Epoch::from(167_u64))).unwrap();

        assert_eq!(first_snapshot.epoch, Epoch::from(164_u64));
        assert_eq!(second_snapshot.epoch, Epoch::from(165_u64));
        assert_eq!(third_snapshot.epoch, Epoch::from(166_u64));
    }

    #[test]
    fn select_bootstrap_snapshots_reports_incomplete_epochs() {
        let snapshots =
            vec![test_snapshot(163, "69206375.hash1", "preprod"), test_snapshot(165, "70070379.hash3", "preprod")];

        let err = select_bootstrap_snapshots(&snapshots, Some(Epoch::from(166_u64))).unwrap_err();
        let err = err.to_string();

        assert!(err.contains("requested 166"));
        assert!(err.contains("none are available"));
    }

    #[test]
    fn select_bootstrap_snapshots_reports_missing_requested_epochs() {
        let snapshots = vec![
            test_snapshot(163, "69206375.hash1", "preprod"),
            test_snapshot(164, "69861372.hash2", "preprod"),
            test_snapshot(165, "70070379.hash3", "preprod"),
        ];

        let err = select_bootstrap_snapshots(&snapshots, Some(Epoch::from(167_u64))).unwrap_err();
        let err = err.to_string();

        assert!(err.contains("requested 167"));
        assert!(err.contains("only 166 is available"));
    }

    #[test]
    fn select_bootstrap_snapshots_reports_too_young_epoch() {
        let snapshots = vec![test_snapshot(1, "69206375.hash1", "preprod")];
        let err = select_bootstrap_snapshots(&snapshots, Some(Epoch::from(2_u64))).unwrap_err();
        assert!(dbg!(err.to_string()).contains("target epoch is too young"));
    }
}
