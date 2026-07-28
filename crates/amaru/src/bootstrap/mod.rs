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

mod chain_sync_client;

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    io,
    path::{Path, PathBuf},
    time::Duration,
};

use amaru_kernel::{
    BlockHeader, Epoch, EraHistory, GlobalParameters, Hash, HeaderHash, IsHeader, NetworkName, Nonce, Peer, Point,
    RawBlock, Slot, StakeCredential, extract_block_header_cbor, from_cbor, num::CheckedSub,
};
use amaru_ledger::{
    bootstrap::import_initial_snapshot,
    store::{EpochTransitionProgress, Store, TransactionalContext},
};
use amaru_observability::{error, info};
use amaru_ouroboros::{ChainStore, Nonces, WriteChainStore};
use amaru_progress_bar::TerminalProgressBar;
use amaru_stores::rocksdb::{RocksDB, RocksDbConfig, consensus::RocksDBStore};
use anyhow::anyhow;
use chain_sync_client::ChainSyncClient;
use pallas_network::{facades::PeerClient, miniprotocols::chainsync::NextResponse};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tar::Archive;
use tokio::{fs, time::timeout};
use zstd::Decoder as ZstdDecoder;

use crate::{
    aws::{AnonymousS3Client, S3Config},
    cardano_node::{parse_state_snapshot_with_nonces, tvar::import_snapshot_from_tvar},
    default_snapshots_dir,
};

/// S3-backed snapshot descriptor used during bootstrap.
#[derive(Debug, Clone)]
struct Snapshot {
    epoch: Epoch,
    point: String,
    /// Full S3 object key: `<network>/<point>.tar.zst`.
    key: String,
}

#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    #[error("Can not create snapshots directory {0}: {1}")]
    CreateSnapshotsDir(PathBuf, io::Error),

    #[error("Unable to store snapshots on disk: {0}")]
    Io(#[from] io::Error),

    #[error("Failed to download snapshot {0}: {1}")]
    DownloadError(String, String),

    #[error("Missing cardano-node snapshot directory {0}")]
    MissingSnapshotDirectory(PathBuf),

    #[error("No bootstrap snapshots found in S3 bucket for this network")]
    NoBootstrapSnapshots,

    #[error(
        "bootstrap target epoch {target_epoch}, but S3 bucket must contain epochs {required_epochs}. Available epochs: {available_epochs}"
    )]
    SnapshotSelectionRequestedEpoch { target_epoch: Epoch, required_epochs: String, available_epochs: String },

    #[error(
        "bootstrap needs the latest 3 consecutive snapshot epochs ending at {latest_epoch}, but S3 bucket only provides epochs {available_epochs}. Required epochs: {required_epochs}"
    )]
    SnapshotSelectionLatestEpoch { latest_epoch: Epoch, required_epochs: String, available_epochs: String },
}

pub const BOOTSTRAP_HEADERS_PER_POINT: usize = 2;
const PACKAGED_BLOCKS_FILE_NAME: &str = "bootstrap.blocks.json";

fn snapshot_directory_path(snapshots_dir: &Path, snapshot: &Snapshot) -> PathBuf {
    snapshots_dir.join(&snapshot.point)
}

fn snapshot_file_path(snapshots_dir: &Path, snapshot: &Snapshot) -> PathBuf {
    snapshots_dir.join(format!("{}.cbor", snapshot.point))
}

fn resolve_snapshot_path(snapshots_dir: &Path, snapshot: &Snapshot) -> Option<PathBuf> {
    let directory = snapshot_directory_path(snapshots_dir, snapshot);
    if node_snapshot_paths(&directory).is_some() {
        return Some(directory);
    }

    let file = snapshot_file_path(snapshots_dir, snapshot);
    is_cbor_snapshot_file(&file).then_some(file)
}

fn snapshot_hash(snapshot: &Snapshot) -> Result<HeaderHash, Box<dyn Error>> {
    match Point::try_from(snapshot.point.as_str())? {
        Point::Specific(_, hash) => Ok(hash),
        Point::Origin => Err("bootstrap snapshots must not use origin".into()),
    }
}

/// List S3 objects under `<network>/`, derive epoch from slot via era history, return `Vec<Snapshot>`.
async fn bootstrap_snapshots(
    network: NetworkName,
    s3: &AnonymousS3Client,
) -> Result<(PathBuf, Vec<Snapshot>), Box<dyn Error>> {
    let snapshots_dir: PathBuf = default_snapshots_dir(network).into();

    let era_history = network
        .as_era_history()
        .ok_or_else(|| format!("no era history available for network {network}; S3 bootstrap is only supported for mainnet, preprod, and preview"))?;

    let s3_snapshots = s3.list_snapshots(network).await?;

    let mut snapshots = Vec::with_capacity(s3_snapshots.len());
    for s3_snap in s3_snapshots {
        let slot = parse_slot_from_point(&s3_snap.point)?;
        let epoch = era_history.slot_to_epoch_unchecked_horizon(Slot::from(slot))?;
        snapshots.push(Snapshot { epoch, point: s3_snap.point, key: s3_snap.key });
    }

    Ok((snapshots_dir, snapshots))
}

/// Parse the slot number from a `<slot>.<hash>` point string.
fn parse_slot_from_point(point: &str) -> Result<u64, Box<dyn Error>> {
    point
        .split('.')
        .next()
        .and_then(|s| s.parse::<u64>().ok())
        .ok_or_else(|| format!("invalid snapshot point format: {point}").into())
}

fn format_epoch_list(epochs: &[Epoch]) -> String {
    if epochs.is_empty() {
        return "none".to_string();
    }

    epochs.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
}

fn select_bootstrap_snapshots(
    snapshots: &[Snapshot],
    target_epoch: Option<Epoch>,
) -> Result<[&Snapshot; 3], Box<dyn Error>> {
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
            let available_epochs = format_epoch_list(&snapshots_by_epoch.keys().copied().collect::<Vec<_>>());
            let required_epochs = format_epoch_list(&required_epochs);

            match target_epoch {
                Some(target_epoch) => Err(BootstrapError::SnapshotSelectionRequestedEpoch {
                    target_epoch,
                    required_epochs,
                    available_epochs,
                }
                .into()),
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
    points: &[Point],
    headers_per_point: usize,
) -> Result<Vec<Vec<u8>>, Box<dyn Error>> {
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
    point: Point,
    headers_per_point: usize,
) -> Result<Vec<Vec<u8>>, Box<dyn Error>> {
    let peer_client = PeerClient::connect(peer_address, network.to_network_magic().as_u64()).await.map_err(|err| {
        error!(bootstrap::peer::FAILED_TO_CONNECT, peer = %peer_address, reason = %err);
        err
    })?;
    let mut client = ChainSyncClient::new(Peer::new(peer_address), peer_client.chainsync, vec![point]);
    let intersection = client.find_intersection().await?;

    info!(bootstrap::headers::FETCH, requested_point = %point, intersection = %intersection, headers_per_point);

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
                let block_header: BlockHeader =
                    from_cbor(&content.cbor).ok_or("failed to decode fetched block header")?;
                let slot = u64::from(block_header.slot());
                headers.push(content.cbor);

                if headers.len() >= headers_per_point || slot == tip.0.slot_or_default() {
                    break;
                }
            }
            NextResponse::RollBackward(point, tip) => {
                info!(bootstrap::fetch::ROLLBACK, ?point, ?tip);
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
    fs::create_dir_all(snapshots_dir)
        .await
        .map_err(|err| BootstrapError::CreateSnapshotsDir(snapshots_dir.to_path_buf(), err))?;

    for snapshot in snapshots {
        let snapshot_dir = snapshot_directory_path(snapshots_dir, snapshot);

        if !should_download_snapshot(snapshots_dir, snapshot) {
            let snapshot_path = resolve_snapshot_path(snapshots_dir, snapshot)
                .unwrap_or_else(|| snapshot_directory_path(snapshots_dir, snapshot));
            info!(bootstrap::snapshot::SKIP_DOWNLOAD, snapshot = %snapshot_path.display());
            continue;
        }

        if snapshot_dir.exists() {
            info!(bootstrap::snapshot::INVALID, snapshot = %snapshot_dir.display());
            fs::remove_dir_all(&snapshot_dir).await?;
        }

        let archive_path = snapshots_dir.join(format!("{}.download.partial", snapshot.point));
        let extract_path = snapshots_dir.join(format!(".{}.extract.partial", snapshot.point));

        info!(bootstrap::snapshot::DOWNLOAD, epoch = %snapshot.epoch, point = %snapshot.point);

        s3.download_object(&snapshot.key, &archive_path)
            .await
            .map_err(|e| BootstrapError::DownloadError(snapshot.key.clone(), e.to_string()))?;

        info!(bootstrap::snapshot::EXTRACT, snapshot = %snapshot_dir.display());

        if let Err(err) = extract_snapshot_archive(&archive_path, &extract_path, &snapshot_dir) {
            let _ = fs::remove_file(&archive_path).await;
            let _ = fs::remove_dir_all(&extract_path).await;
            return Err(err);
        }

        fs::remove_file(&archive_path).await?;
    }

    Ok(())
}

fn extract_snapshot_archive(
    archive_path: &Path,
    extract_path: &Path,
    snapshot_dir: &Path,
) -> Result<(), BootstrapError> {
    if extract_path.exists() {
        std::fs::remove_dir_all(extract_path)?;
    }

    std::fs::create_dir_all(extract_path)?;

    let archive_file = std::fs::File::open(archive_path)?;
    let mut archive = Archive::new(ZstdDecoder::new(archive_file)?);
    archive.unpack(extract_path)?;

    let extracted_dir = find_extracted_snapshot_dir(extract_path)?
        .ok_or_else(|| BootstrapError::MissingSnapshotDirectory(snapshot_dir.to_path_buf()))?;

    if extracted_dir == extract_path {
        std::fs::rename(extract_path, snapshot_dir)?;
        return Ok(());
    }

    std::fs::rename(&extracted_dir, snapshot_dir)?;
    std::fs::remove_dir_all(extract_path)?;

    Ok(())
}

fn find_extracted_snapshot_dir(path: &Path) -> Result<Option<PathBuf>, io::Error> {
    if node_snapshot_paths(path).is_some() {
        return Ok(Some(path.to_path_buf()));
    }

    let snapshot_dirs = std::fs::read_dir(path)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|child| node_snapshot_paths(child).is_some())
        .collect::<Vec<_>>();

    match snapshot_dirs.as_slice() {
        [] => Ok(None),
        [snapshot_dir] => Ok(Some(snapshot_dir.clone())),
        _ => Err(io::Error::other(format!("multiple snapshot directories extracted from {}", path.display()))),
    }
}

/// Set the internal dbs in such a state that amaru can run
pub async fn bootstrap(
    network: NetworkName,
    global_parameters: &GlobalParameters,
    ledger_dir: PathBuf,
    chain_dir: PathBuf,
    target_epoch: Option<Epoch>,
    s3_config: S3Config,
) -> Result<(), Box<dyn Error>> {
    let s3 = AnonymousS3Client::new(s3_config);
    let (snapshots_dir, snapshots) = bootstrap_snapshots(network, &s3).await?;
    let [first_snapshot, second_snapshot, third_snapshot] = select_bootstrap_snapshots(&snapshots, target_epoch)?;

    download_snapshots(&[first_snapshot, second_snapshot, third_snapshot], &snapshots_dir, &s3).await?;

    let first_snapshot_path = resolve_snapshot_path(&snapshots_dir, first_snapshot).ok_or_else(|| {
        BootstrapError::MissingSnapshotDirectory(snapshot_directory_path(&snapshots_dir, first_snapshot))
    })?;
    let second_snapshot_path = resolve_snapshot_path(&snapshots_dir, second_snapshot).ok_or_else(|| {
        BootstrapError::MissingSnapshotDirectory(snapshot_directory_path(&snapshots_dir, second_snapshot))
    })?;
    let third_snapshot_path = resolve_snapshot_path(&snapshots_dir, third_snapshot).ok_or_else(|| {
        BootstrapError::MissingSnapshotDirectory(snapshot_directory_path(&snapshots_dir, third_snapshot))
    })?;

    let mut recently_unregistered_accounts = BTreeSet::new();

    import_snapshot(network, global_parameters, &first_snapshot_path, &ledger_dir, &mut recently_unregistered_accounts)
        .await?;

    import_snapshot(
        network,
        global_parameters,
        &second_snapshot_path,
        &ledger_dir,
        &mut recently_unregistered_accounts,
    )
    .await?;

    let imported_third_snapshot = import_snapshot_with_optional_nonces(
        network,
        global_parameters,
        &third_snapshot_path,
        &ledger_dir,
        Some(snapshot_hash(second_snapshot)?),
        &mut recently_unregistered_accounts,
    )
    .await?;

    let chain_db = RocksDBStore::create(RocksDbConfig::new(chain_dir.clone()))?;
    let initial_nonces =
        imported_third_snapshot.initial_nonces.ok_or("bootstrap import must produce nonces for the latest snapshot")?;
    store_nonces(imported_third_snapshot.epoch, &chain_db, initial_nonces)?;
    let blocks = load_packaged_blocks_for_bootstrap(&second_snapshot_path, &third_snapshot_path)?;
    import_packaged_blocks(&chain_db, blocks).await?;

    Ok(())
}

pub async fn import_packaged_blocks(db: &RocksDBStore, blocks: Vec<Vec<u8>>) -> Result<(), Box<dyn Error>> {
    for block in blocks {
        let header_cbor = extract_block_header_cbor(&block)?;
        let block_header: BlockHeader =
            from_cbor(header_cbor).ok_or("failed to decode packaged bootstrap block header")?;
        let hash = block_header.hash();

        info!(bootstrap::header::IMPORT, header = %hash);

        db.store_header(&block_header)?;
        db.store_block(&hash, &RawBlock::from(block.as_slice()))?;
    }

    Ok(())
}

fn load_packaged_blocks_for_bootstrap(
    second_snapshot_path: &Path,
    third_snapshot_path: &Path,
) -> Result<Vec<Vec<u8>>, Box<dyn Error>> {
    let mut blocks = load_packaged_blocks_from_snapshot(second_snapshot_path)?;
    blocks.extend(load_packaged_blocks_from_snapshot(third_snapshot_path)?);
    Ok(blocks)
}

fn load_packaged_blocks_from_snapshot(snapshot_path: &Path) -> Result<Vec<Vec<u8>>, Box<dyn Error>> {
    let blocks_file = snapshot_path.join(PACKAGED_BLOCKS_FILE_NAME);
    if !blocks_file.is_file() {
        return Err(format!(
            "missing packaged bootstrap blocks at {}. Re-generate snapshots with `amaru create-bootstrap-snapshots`.",
            blocks_file.display()
        )
        .into());
    }

    let hex_blocks: Vec<String> = serde_json::from_slice(&std::fs::read(&blocks_file)?)?;
    if hex_blocks.len() < BOOTSTRAP_HEADERS_PER_POINT {
        return Err(format!(
            "packaged bootstrap blocks at {} contain {} blocks; expected at least {}.",
            blocks_file.display(),
            hex_blocks.len(),
            BOOTSTRAP_HEADERS_PER_POINT
        )
        .into());
    }

    let mut blocks = Vec::with_capacity(hex_blocks.len());
    for hex_block in hex_blocks {
        blocks.push(hex::decode(hex_block)?);
    }

    Ok(blocks)
}

fn deserialize_point<'de, D>(deserializer: D) -> Result<Point, D::Error>
where
    D: Deserializer<'de>,
{
    let buf = <&str>::deserialize(deserializer)?;
    Point::try_from(buf).map_err(|e| serde::de::Error::custom(format!("cannot convert vector to point: {:?}", e)))
}

fn serialize_point<S: Serializer>(point: &Point, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&point.to_string())
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InitialNonces {
    #[serde(serialize_with = "serialize_point", deserialize_with = "deserialize_point")]
    pub at: Point,
    pub active: Nonce,
    pub evolving: Nonce,
    pub candidate: Nonce,
    pub tail: HeaderHash,
}

pub fn store_nonces(epoch: Epoch, db: &dyn ChainStore, initial_nonces: InitialNonces) -> Result<(), Box<dyn Error>> {
    let header_hash = Hash::from(&initial_nonces.at);

    info!(bootstrap::nonces::IMPORT, point = %initial_nonces.at);

    let nonces = Nonces {
        epoch,
        active: initial_nonces.active,
        evolving: initial_nonces.evolving,
        candidate: initial_nonces.candidate,
        tail: initial_nonces.tail,
    };

    db.put_nonces(&header_hash, &nonces)?;

    Ok(())
}

pub async fn import_headers(db: &RocksDBStore, headers: Vec<Vec<u8>>) -> Result<(), Box<dyn Error>> {
    for header in headers {
        let block_header: BlockHeader = from_cbor(&header).ok_or("failed to decode packaged bootstrap header")?;
        let hash = block_header.hash();

        info!(bootstrap::header::IMPORT, header = %hash);

        db.store_header(&block_header)?;
    }

    Ok(())
}

pub async fn import_snapshots_from_directory(
    network: NetworkName,
    global_parameters: &GlobalParameters,
    ledger_dir: &Path,
    snapshot_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if node_snapshot_paths(snapshot_dir).is_some() {
        let snapshots = [snapshot_dir.to_path_buf()];
        return import_snapshots(network, global_parameters, &snapshots, ledger_dir).await;
    }

    let mut snapshots = std::fs::read_dir(snapshot_dir)?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| node_snapshot_paths(path).is_some())
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
) -> Result<(), Box<dyn std::error::Error>> {
    info!(bootstrap::snapshots::IMPORT, count = snapshots.len());
    let mut recently_unregistered_accounts: BTreeSet<StakeCredential> = BTreeSet::new();
    for snapshot in snapshots {
        import_snapshot(network, global_parameters, snapshot, ledger_dir, &mut recently_unregistered_accounts).await?;
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("malformed snapshot point in file name: {0}")]
    MalformedDate(String),
    #[error("invalid snapshot file: {0}")]
    InvalidSnapshotFile(PathBuf),
    #[error(
        "expected cardano-node InMem snapshot directory with `state` and `tables/tvar`, or a `.cbor` snapshot file: {0}"
    )]
    UnsupportedSnapshotPath(PathBuf),
}

struct ImportedSnapshot {
    epoch: Epoch,
    initial_nonces: Option<InitialNonces>,
}

async fn import_snapshot(
    network: NetworkName,
    global_parameters: &GlobalParameters,
    snapshot: &Path,
    ledger_dir: &Path,
    recently_unregistered_accounts: &mut BTreeSet<StakeCredential>,
) -> Result<(), Box<dyn std::error::Error>> {
    import_snapshot_with_optional_nonces(
        network,
        global_parameters,
        snapshot,
        ledger_dir,
        None,
        recently_unregistered_accounts,
    )
    .await?;
    Ok(())
}

async fn import_snapshot_with_optional_nonces(
    network: NetworkName,
    global_parameters: &GlobalParameters,
    snapshot: &Path,
    ledger_dir: &Path,
    nonce_tail: Option<HeaderHash>,
    recently_unregistered_accounts: &mut BTreeSet<StakeCredential>,
) -> Result<ImportedSnapshot, Box<dyn std::error::Error>> {
    if let Some(paths) = node_snapshot_paths(snapshot) {
        return import_node_snapshot_dir(
            network,
            global_parameters,
            snapshot,
            &paths,
            ledger_dir,
            nonce_tail,
            recently_unregistered_accounts,
        )
        .await;
    }

    if is_cbor_snapshot_file(snapshot) {
        return import_cbor_snapshot_file(
            network,
            global_parameters,
            snapshot,
            ledger_dir,
            nonce_tail,
            recently_unregistered_accounts,
        )
        .await;
    }

    Err(Box::new(ImportError::UnsupportedSnapshotPath(snapshot.to_path_buf())))
}

#[expect(clippy::unwrap_used)]
async fn import_cbor_snapshot_file(
    network: NetworkName,
    global_parameters: &GlobalParameters,
    snapshot: &Path,
    ledger_dir: &Path,
    nonce_tail: Option<HeaderHash>,
    recently_unregistered_accounts: &mut BTreeSet<StakeCredential>,
) -> Result<ImportedSnapshot, Box<dyn std::error::Error>> {
    info!(bootstrap::snapshot::IMPORT_FILE, path = %snapshot.display());

    let point =
        Point::try_from(snapshot.file_stem().and_then(|s| s.to_str()).unwrap()).map_err(ImportError::MalformedDate)?;
    let dir = snapshot.parent().ok_or_else(|| ImportError::InvalidSnapshotFile(snapshot.to_path_buf()))?;
    let era_history = make_era_history(dir, &point, network)?;
    let initial_nonces = if let Some(tail) = nonce_tail {
        let bytes = std::fs::read(snapshot)?;
        let (_, initial_nonces) =
            parse_state_snapshot_with_nonces(minicbor::Decoder::new(&bytes), global_parameters, tail)?;
        Some(initial_nonces)
    } else {
        None
    };

    std::fs::create_dir_all(ledger_dir)?;

    if std::fs::exists(ledger_dir.join("live"))? {
        std::fs::remove_dir_all(ledger_dir.join("live"))?;
    }

    let db = RocksDB::empty(&RocksDbConfig::new(ledger_dir.to_path_buf()))?;
    let mut file = std::fs::File::open(snapshot)?;

    let builder = std::thread::Builder::new().stack_size(10_000_000);

    let mut accounts = recently_unregistered_accounts.clone();

    let (db, epoch, accounts) = builder
        .spawn(move || {
            import_initial_snapshot(&db, &mut file, &mut accounts, &point, &era_history, network, |size, template| {
                TerminalProgressBar::new(size as u64, template).boxed()
            })
            .map_err(|e| e.to_string())
            .map(|epoch| (db, epoch, accounts))
        })
        .unwrap()
        .join()
        .unwrap()?;

    *recently_unregistered_accounts = accounts;

    db.next_snapshot(epoch)?;

    db.with_transaction(|batch| batch.try_epoch_transition(None, Some(EpochTransitionProgress::SnapshotTaken)))?;

    Ok(ImportedSnapshot { epoch, initial_nonces })
}

#[expect(clippy::unwrap_used)]
async fn import_node_snapshot_dir(
    network: NetworkName,
    global_parameters: &GlobalParameters,
    snapshot_dir: &Path,
    paths: &NodeSnapshotPaths,
    ledger_dir: &Path,
    nonce_tail: Option<HeaderHash>,
    recently_unregistered_accounts: &mut BTreeSet<StakeCredential>,
) -> Result<ImportedSnapshot, Box<dyn std::error::Error>> {
    info!(bootstrap::snapshot::IMPORT_DIR, path = %snapshot_dir.display());

    std::fs::create_dir_all(ledger_dir)?;

    if std::fs::exists(ledger_dir.join("live"))? {
        std::fs::remove_dir_all(ledger_dir.join("live"))?;
    }

    let db = RocksDB::empty(&RocksDbConfig::new(ledger_dir.to_path_buf()))?;
    let mut state_file = std::fs::File::open(&paths.state)?;
    let mut utxo_file = std::fs::File::open(&paths.utxo)?;

    let global_parameters = global_parameters.clone();
    let builder = std::thread::Builder::new().stack_size(10_000_000);
    let mut accounts = recently_unregistered_accounts.clone();

    let (db, epoch, initial_nonces, accounts) = builder
        .spawn(move || {
            import_snapshot_from_tvar(
                &db,
                &mut state_file,
                &mut utxo_file,
                network,
                &global_parameters,
                nonce_tail,
                &mut accounts,
                |size, template| TerminalProgressBar::new(size as u64, template).boxed(),
            )
            .map_err(|e| e.to_string())
            .map(|(epoch, _point, initial_nonces)| (db, epoch, initial_nonces, accounts))
        })
        .unwrap()
        .join()
        .unwrap()?;

    *recently_unregistered_accounts = accounts;

    db.next_snapshot(epoch)?;

    db.with_transaction(|batch| batch.try_epoch_transition(None, Some(EpochTransitionProgress::SnapshotTaken)))?;

    Ok(ImportedSnapshot { epoch, initial_nonces })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NodeSnapshotPaths {
    state: PathBuf,
    utxo: PathBuf,
}

fn node_snapshot_paths(path: &Path) -> Option<NodeSnapshotPaths> {
    if !path.is_dir() {
        return None;
    }

    let state = path.join("state");
    let utxo = path.join("tables").join("tvar");

    if state.is_file() && utxo.is_file() { Some(NodeSnapshotPaths { state, utxo }) } else { None }
}

fn is_cbor_snapshot_file(path: &Path) -> bool {
    path.is_file() && path.extension().and_then(|extension| extension.to_str()) == Some("cbor")
}

// TODO: See if this cannot be determined from the snapshot?
fn make_era_history(dir: &Path, point: &Point, network: NetworkName) -> Result<EraHistory, Box<dyn std::error::Error>> {
    network.as_era_history().cloned().map(Ok).unwrap_or_else(|| {
        let filename = format!("history.{}.{}.json", point.slot_or_default(), point.hash());
        let history_file = dir.join(filename);
        if !history_file.is_file() {
            return Err(format!("cannot import testnet era history from {}", history_file.display()).into());
        }

        Ok(serde_json::from_slice(&std::fs::read(&history_file)?)?)
    })
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, time::Duration};

    use amaru_kernel::{Epoch, EraBound, EraHistory, EraName, EraParams, EraSummary, HeaderHash, Slot};
    use tempfile::tempdir;

    use super::{
        Snapshot, is_cbor_snapshot_file, node_snapshot_paths, select_bootstrap_snapshots, should_download_snapshot,
        sort_snapshots_by_slot,
    };
    use crate::cardano_node::ParsedStateSnapshot;

    fn test_snapshot(epoch: u64, point: &str, network: &str) -> Snapshot {
        Snapshot { epoch: Epoch::from(epoch), point: point.to_string(), key: format!("{network}/{point}.tar.zst") }
    }

    fn snapshot_epoch(parsed_snapshot: &ParsedStateSnapshot) -> Result<Epoch, Box<dyn std::error::Error>> {
        Ok(parsed_snapshot.era_history.slot_to_epoch_unchecked_horizon(parsed_snapshot.slot.into())?)
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
            era_history,
            ledger_data_begin: 0,
            ledger_data_end: 0,
        };

        assert_eq!(snapshot_epoch(&parsed_snapshot).unwrap(), Epoch::from(12_u64));
    }

    #[test]
    fn node_snapshot_paths_requires_nested_tvar() {
        let temp_dir = tempdir().unwrap();
        let snapshot_dir = temp_dir.path().join("69206375.hash");
        std::fs::create_dir_all(&snapshot_dir).unwrap();
        std::fs::write(snapshot_dir.join("state"), b"state").unwrap();
        std::fs::write(snapshot_dir.join("tables"), b"utxo").unwrap();

        assert!(node_snapshot_paths(&snapshot_dir).is_none());
    }

    #[test]
    fn should_download_snapshot_for_invalid_existing_directory() {
        let temp_dir = tempdir().unwrap();
        let snapshots_dir = temp_dir.path();
        let snapshot = test_snapshot(163, "69206375.hash", "preprod");
        let snapshot_dir = snapshots_dir.join(&snapshot.point);
        std::fs::create_dir_all(&snapshot_dir).unwrap();
        std::fs::write(snapshot_dir.join("state"), b"state").unwrap();
        std::fs::write(snapshot_dir.join("tables"), b"utxo").unwrap();

        assert!(should_download_snapshot(snapshots_dir, &snapshot));
    }

    #[test]
    fn should_not_download_valid_tvar_snapshot_directory() {
        let temp_dir = tempdir().unwrap();
        let snapshots_dir = temp_dir.path();
        let snapshot = test_snapshot(163, "69206375.hash", "preprod");
        let snapshot_dir = snapshots_dir.join(&snapshot.point);
        std::fs::create_dir_all(snapshot_dir.join("tables")).unwrap();
        std::fs::write(snapshot_dir.join("state"), b"state").unwrap();
        std::fs::write(snapshot_dir.join("tables").join("tvar"), b"utxo").unwrap();

        assert!(!should_download_snapshot(snapshots_dir, &snapshot));
    }

    #[test]
    fn should_not_download_existing_cbor_snapshot_file() {
        let temp_dir = tempdir().unwrap();
        let snapshots_dir = temp_dir.path();
        let snapshot = test_snapshot(163, "69206375.hash", "preprod");
        let snapshot_file = snapshots_dir.join("69206375.hash.cbor");
        std::fs::write(&snapshot_file, b"snapshot").unwrap();

        assert!(is_cbor_snapshot_file(&snapshot_file));
        assert!(!should_download_snapshot(snapshots_dir, &snapshot));
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
    fn select_bootstrap_snapshots_reports_missing_requested_epochs() {
        let snapshots =
            vec![test_snapshot(163, "69206375.hash1", "preprod"), test_snapshot(165, "70070379.hash3", "preprod")];

        let err = select_bootstrap_snapshots(&snapshots, Some(Epoch::from(166_u64))).unwrap_err();
        let err = err.to_string();

        assert!(err.contains("target epoch 166"));
        assert!(err.contains("must contain epochs 163, 164, 165"));
        assert!(err.contains("Available epochs: 163, 165"));
    }

    #[test]
    fn select_bootstrap_snapshots_reports_too_young_epoch() {
        let snapshots = vec![test_snapshot(1, "69206375.hash1", "preprod")];
        let err = select_bootstrap_snapshots(&snapshots, Some(Epoch::from(2_u64))).unwrap_err();
        assert!(dbg!(err.to_string()).contains("target epoch is too young"));
    }
}
