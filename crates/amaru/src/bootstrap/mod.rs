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
    collections::BTreeMap,
    error::Error,
    io,
    path::{Path, PathBuf},
    time::Duration,
};

use amaru_kernel::{
    BlockHeader, Epoch, EraHistory, GlobalParameters, Hash, HeaderHash, IsHeader, NetworkName, Nonce, Peer, Point,
    from_cbor,
};
use amaru_ledger::{
    bootstrap::import_initial_snapshot,
    store::{EpochTransitionProgress, Store, TransactionalContext},
};
use amaru_ouroboros::{ChainStore, Nonces, WriteChainStore};
use amaru_progress_bar::{ProgressBar, TerminalProgressBar};
use amaru_stores::rocksdb::{RocksDB, RocksDbConfig, consensus::RocksDBStore};
use anyhow::anyhow;
use async_compression::tokio::bufread::GzipDecoder as AsyncGzipDecoder;
use chain_sync_client::ChainSyncClient;
use flate2::read::GzDecoder;
use futures_util::TryStreamExt;
use num::CheckedSub;
use pallas_network::{facades::PeerClient, miniprotocols::chainsync::NextResponse};
use reqwest::StatusCode;
use tar::Archive;
use tokio::{
    fs::{self, File},
    io::{AsyncWriteExt, BufReader},
    time::timeout,
};
use tokio_util::io::StreamReader;
use tracing::{error, info};

use crate::{
    cardano_node::{ParsedStateSnapshot, parse_state_snapshot_with_nonces, tvar::import_snapshot_from_tvar},
    default_data_dir, default_snapshots_dir, get_bootstrap_file,
};

/// Configuration for a single ledger state's snapshot to be imported.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
struct Snapshot {
    /// The snapshot's epoch.
    epoch: Epoch,

    /// The snapshot's point, typically the last point of an epoch.
    point: Point,

    /// The parent point of the snapshot's point.
    parent_point: Point,

    /// The URL to retrieve snapshot from.
    #[serde(default)]
    url: String,
}

#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    #[error("Missing configuration file {0}")]
    MissingConfigFile(PathBuf),

    #[error("Failed to parse snapshots JSON file {}: {source}", path.display())]
    MalformedSnapshotsFile {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error("Can not create snapshots directory {0}: {1}")]
    CreateSnapshotsDir(PathBuf, io::Error),

    #[error("Unable to store snapshots on disk: {0}")]
    Io(#[from] io::Error),

    #[error("Failed to download snapshot at url {0}: {1}")]
    DownloadError(String, reqwest::Error),

    #[error("Failed to download snapshot from {0}: HTTP status code {1}")]
    DownloadInvalidStatusCode(String, StatusCode),

    #[error("Missing cardano-node snapshot directory {0}")]
    MissingSnapshotDirectory(PathBuf),

    #[error("Missing bootstrap parent point for snapshot {0}")]
    MissingParentPoint(Point),

    #[error("No bootstrap snapshots are configured in snapshots.json")]
    NoBootstrapSnapshots,

    #[error(
        "bootstrap target epoch {target_epoch}, but snapshots.json must contain epochs {required_epochs}. Available epochs: {available_epochs}"
    )]
    SnapshotSelectionRequestedEpoch { target_epoch: Epoch, required_epochs: String, available_epochs: String },

    #[error(
        "bootstrap needs the latest 3 consecutive snapshot epochs ending at {latest_epoch}, but snapshots.json only provides epochs {available_epochs}. Required epochs: {required_epochs}"
    )]
    SnapshotSelectionLatestEpoch { latest_epoch: Epoch, required_epochs: String, available_epochs: String },
}

pub const BOOTSTRAP_HEADERS_PER_POINT: usize = 2;
const PACKAGED_HEADERS_FILE_NAME: &str = "bootstrap.headers.json";

fn snapshot_directory_path(snapshots_dir: &Path, snapshot: &Snapshot) -> PathBuf {
    snapshots_dir.join(snapshot.point.to_string())
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

fn load_local_epoch_snapshots(network: NetworkName) -> Vec<Snapshot> {
    let metadata_dir = PathBuf::from(default_data_dir(network)).join("epoch-snapshots").join("epochs");
    if !metadata_dir.is_dir() {
        return Vec::new();
    }

    let Ok(entries) = std::fs::read_dir(&metadata_dir) else {
        return Vec::new();
    };

    entries
        .flatten()
        .filter(|entry| entry.path().extension().and_then(|e| e.to_str()) == Some("json"))
        .filter_map(|entry| std::fs::read(entry.path()).ok())
        .filter_map(|bytes| serde_json::from_slice::<Snapshot>(&bytes).ok())
        .collect()
}

fn bootstrap_snapshots(network: NetworkName) -> Result<(PathBuf, Vec<Snapshot>), Box<dyn Error>> {
    let snapshot_file_name = "snapshots.json";
    let snapshots_dir: PathBuf = default_snapshots_dir(network).into();
    let snapshots_file = get_bootstrap_file(network, snapshot_file_name)?
        .ok_or(BootstrapError::MissingConfigFile(snapshot_file_name.into()))?;
    let mut snapshots: Vec<Snapshot> = serde_json::from_slice(&snapshots_file)
        .map_err(|source| BootstrapError::MalformedSnapshotsFile { path: snapshot_file_name.into(), source })?;

    let local_snapshots = load_local_epoch_snapshots(network);
    if !local_snapshots.is_empty() {
        info!(count = local_snapshots.len(), "detected locally-created snapshots from create-snapshots");
        for local_snapshot in local_snapshots {
            if !snapshots.iter().any(|s| s.epoch == local_snapshot.epoch) {
                snapshots.push(local_snapshot);
            }
        }
    }

    Ok((snapshots_dir, snapshots))
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
    let snapshots_by_epoch = snapshots.iter().map(|snapshot| (snapshot.epoch, snapshot)).collect::<BTreeMap<_, _>>();
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
            let available_epochs = snapshots_by_epoch.keys().copied().collect::<Vec<_>>();
            let available_epochs = format_epoch_list(&available_epochs);
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

fn initial_nonces_from_snapshot(
    snapshot_path: &Path,
    global_parameters: &GlobalParameters,
    tail: HeaderHash,
) -> Result<(Epoch, InitialNonces), Box<dyn Error>> {
    let bytes = if let Some(snapshot_paths) = node_snapshot_paths(snapshot_path) {
        std::fs::read(snapshot_paths.state)?
    } else if is_cbor_snapshot_file(snapshot_path) {
        std::fs::read(snapshot_path)?
    } else {
        return Err(Box::new(ImportError::UnsupportedSnapshotPath(snapshot_path.to_path_buf())));
    };

    let (parsed_snapshot, initial_nonces) =
        parse_state_snapshot_with_nonces(minicbor::Decoder::new(&bytes), global_parameters, tail)?;
    let epoch = snapshot_epoch(&parsed_snapshot)?;

    Ok((epoch, initial_nonces))
}

fn default_bootstrap_nonces_from_snapshots(
    snapshots_dir: &Path,
    snapshots: &[Snapshot],
    global_parameters: &GlobalParameters,
) -> Result<(Epoch, InitialNonces), Box<dyn Error>> {
    let [_, second_snapshot, third_snapshot] = select_bootstrap_snapshots(snapshots, None)?;
    let third_snapshot_path = resolve_snapshot_path(snapshots_dir, third_snapshot).ok_or_else(|| {
        BootstrapError::MissingSnapshotDirectory(snapshot_directory_path(snapshots_dir, third_snapshot))
    })?;
    let (epoch, initial_nonces) =
        initial_nonces_from_snapshot(&third_snapshot_path, global_parameters, second_snapshot.point.hash())?;

    Ok((epoch, initial_nonces))
}

pub fn default_bootstrap_nonces(
    network: NetworkName,
    global_parameters: &GlobalParameters,
) -> Result<(Epoch, InitialNonces), Box<dyn Error>> {
    let (snapshots_dir, snapshots) = bootstrap_snapshots(network)?;
    default_bootstrap_nonces_from_snapshots(&snapshots_dir, &snapshots, global_parameters)
}

fn bootstrap_parent_points(snapshots: [&Snapshot; 3]) -> Vec<Point> {
    vec![snapshots[1].parent_point, snapshots[2].parent_point]
}

pub fn default_bootstrap_parent_points(network: NetworkName) -> Result<Vec<Point>, Box<dyn Error>> {
    let (_, snapshots) = bootstrap_snapshots(network)?;
    Ok(bootstrap_parent_points(select_bootstrap_snapshots(&snapshots, None)?))
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
        error!(peer = %peer_address, reason = %err, "failed to connect to peer");
        err
    })?;
    let mut client = ChainSyncClient::new(Peer::new(peer_address), peer_client.chainsync, vec![point]);
    let intersection = client.find_intersection().await?;

    info!(requested_point = %point, intersection = %intersection, headers_per_point, "fetching bootstrap headers from peer");

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
                info!(?point, ?tip, "roll backward while fetching bootstrap headers");
            }
            NextResponse::Await => continue,
        }
    }

    info!(requested_point = %point, total = headers.len(), "fetched bootstrap headers from peer");

    Ok(headers)
}

async fn download_snapshots(snapshots: &[&Snapshot], snapshots_dir: &Path) -> Result<(), BootstrapError> {
    fs::create_dir_all(snapshots_dir)
        .await
        .map_err(|err| BootstrapError::CreateSnapshotsDir(snapshots_dir.to_path_buf(), err))?;

    let client = reqwest::Client::new();

    for snapshot in snapshots {
        let snapshot_dir = snapshot_directory_path(snapshots_dir, snapshot);
        let snapshot_file = snapshot_file_path(snapshots_dir, snapshot);
        if let Some(snapshot_path) = resolve_snapshot_path(snapshots_dir, snapshot) {
            info!(snapshot = %snapshot_path.display(), "snapshot already exists, skipping download");
            continue;
        }

        if snapshot_dir.exists() {
            info!(snapshot = %snapshot_dir.display(), "snapshot directory exists but is not a valid tvar snapshot, removing it");
            fs::remove_dir_all(&snapshot_dir).await?;
        }

        if snapshot_file.exists() && !is_cbor_snapshot_file(&snapshot_file) {
            info!(snapshot = %snapshot_file.display(), "snapshot file exists but is not a valid cbor snapshot, removing it");
            fs::remove_file(&snapshot_file).await?;
        }

        info!(epoch = %snapshot.epoch, point = %snapshot.point, "downloading snapshot");

        if snapshot.url.ends_with(".cbor.gz") {
            let (tmp_path, mut file) = create_partial_file(&snapshot_file).await?;
            download_gzip_to_file(&mut file, response_from_snapshot(&client, snapshot).await?).await?;
            file.sync_all().await?;
            drop(file);
            fs::rename(&tmp_path, &snapshot_file).await?;
            info!(snapshot = %snapshot_file.display(), "downloaded snapshot");
            continue;
        }

        let archive_path = snapshots_dir.join(format!("{}.download.partial", snapshot.point));
        let extract_path = snapshots_dir.join(format!(".{}.extract.partial", snapshot.point));

        let response = response_from_snapshot(&client, snapshot).await?;

        let mut file = File::create(&archive_path).await?;
        download_to_file(&mut file, response).await?;
        file.sync_all().await?;
        drop(file);

        info!(snapshot = %snapshot_dir.display(), "extracting archive");

        if let Err(err) = extract_snapshot_archive(&archive_path, &extract_path, &snapshot_dir) {
            let _ = fs::remove_file(&archive_path).await;
            let _ = fs::remove_dir_all(&extract_path).await;
            return Err(err);
        }

        fs::remove_file(&archive_path).await?;

        info!(snapshot = %snapshot_dir.display(), "downloaded snapshot");
    }

    Ok(())
}

async fn response_from_snapshot(
    client: &reqwest::Client,
    snapshot: &Snapshot,
) -> Result<reqwest::Response, BootstrapError> {
    let response = client
        .get(&snapshot.url)
        .send()
        .await
        .map_err(|err| BootstrapError::DownloadError(snapshot.url.clone(), err))?;

    if !response.status().is_success() {
        return Err(BootstrapError::DownloadInvalidStatusCode(snapshot.url.clone(), response.status()));
    }

    Ok(response)
}

async fn create_partial_file(target_path: &Path) -> Result<(PathBuf, File), BootstrapError> {
    let tmp_path = target_path.with_extension("partial");
    let file = File::create(&tmp_path).await?;
    Ok((tmp_path, file))
}

async fn download_to_file(file: &mut File, response: reqwest::Response) -> Result<(), BootstrapError> {
    let progress = new_download_progress_bar(response.content_length());
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.try_next().await.map_err(io::Error::other)? {
        progress.tick(chunk.len());
        file.write_all(&chunk).await?;
    }

    progress.clear();

    Ok(())
}

async fn download_gzip_to_file(file: &mut File, response: reqwest::Response) -> Result<(), BootstrapError> {
    let progress = new_download_progress_bar(response.content_length());
    let raw_stream_reader = StreamReader::new(
        response.bytes_stream().inspect_ok(|chunk| progress.tick(chunk.len())).map_err(io::Error::other),
    );
    let buffered_reader = BufReader::new(raw_stream_reader);
    let mut decoded_stream = AsyncGzipDecoder::new(buffered_reader);
    tokio::io::copy(&mut decoded_stream, file).await?;
    progress.clear();
    Ok(())
}

#[allow(clippy::expect_used)]
fn new_download_progress_bar(content_length: Option<u64>) -> impl ProgressBar {
    TerminalProgressBar::new(
        content_length.unwrap_or(0),
        "Downloading [{bytes:>10}/{total_bytes:<10}] {bar:40.green} {bytes_per_sec:>12} ({eta} remaining)",
    )
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
    let mut archive = Archive::new(GzDecoder::new(archive_file));
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
) -> Result<(), Box<dyn Error>> {
    let (snapshots_dir, snapshots) = bootstrap_snapshots(network)?;
    let [first_snapshot, second_snapshot, third_snapshot] = select_bootstrap_snapshots(&snapshots, target_epoch)?;

    download_snapshots(&[first_snapshot, second_snapshot, third_snapshot], &snapshots_dir).await?;

    let first_snapshot_path = resolve_snapshot_path(&snapshots_dir, first_snapshot).ok_or_else(|| {
        BootstrapError::MissingSnapshotDirectory(snapshot_directory_path(&snapshots_dir, first_snapshot))
    })?;
    let second_snapshot_path = resolve_snapshot_path(&snapshots_dir, second_snapshot).ok_or_else(|| {
        BootstrapError::MissingSnapshotDirectory(snapshot_directory_path(&snapshots_dir, second_snapshot))
    })?;
    let third_snapshot_path = resolve_snapshot_path(&snapshots_dir, third_snapshot).ok_or_else(|| {
        BootstrapError::MissingSnapshotDirectory(snapshot_directory_path(&snapshots_dir, third_snapshot))
    })?;

    import_snapshot(network, global_parameters, &first_snapshot_path, &ledger_dir).await?;
    import_snapshot(network, global_parameters, &second_snapshot_path, &ledger_dir).await?;
    let imported_third_snapshot = import_snapshot_with_optional_nonces(
        network,
        global_parameters,
        &third_snapshot_path,
        &ledger_dir,
        Some(second_snapshot.point.hash()),
    )
    .await?;

    let chain_db = RocksDBStore::open_and_migrate(&RocksDbConfig::new(chain_dir.clone()))?;
    let initial_nonces =
        imported_third_snapshot.initial_nonces.ok_or("bootstrap import must produce nonces for the latest snapshot")?;
    store_nonces(imported_third_snapshot.epoch, &chain_db, initial_nonces)?;
    let headers = load_packaged_headers_for_bootstrap(&second_snapshot_path, &third_snapshot_path)?;
    import_headers(&chain_db, headers).await?;

    Ok(())
}

fn load_packaged_headers_for_bootstrap(
    second_snapshot_path: &Path,
    third_snapshot_path: &Path,
) -> Result<Vec<Vec<u8>>, Box<dyn Error>> {
    let mut headers = load_packaged_headers_from_snapshot(second_snapshot_path)?;
    headers.extend(load_packaged_headers_from_snapshot(third_snapshot_path)?);
    Ok(headers)
}

fn load_packaged_headers_from_snapshot(snapshot_path: &Path) -> Result<Vec<Vec<u8>>, Box<dyn Error>> {
    let headers_file = snapshot_path.join(PACKAGED_HEADERS_FILE_NAME);
    if !headers_file.is_file() {
        return Err(format!(
            "missing packaged bootstrap headers at {}. Re-generate snapshots with `amaru create-snapshots`.",
            headers_file.display()
        )
        .into());
    }

    let hex_headers: Vec<String> = serde_json::from_slice(&std::fs::read(&headers_file)?)?;
    if hex_headers.len() < BOOTSTRAP_HEADERS_PER_POINT {
        return Err(format!(
            "packaged bootstrap headers at {} contain {} headers; expected at least {}.",
            headers_file.display(),
            hex_headers.len(),
            BOOTSTRAP_HEADERS_PER_POINT
        )
        .into());
    }

    let mut headers = Vec::with_capacity(hex_headers.len());
    for hex_header in hex_headers {
        headers.push(hex::decode(hex_header)?);
    }

    Ok(headers)
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct InitialNonces {
    pub at: Point,
    pub active: Nonce,
    pub evolving: Nonce,
    pub candidate: Nonce,
    pub tail: HeaderHash,
}

fn snapshot_epoch(parsed_snapshot: &ParsedStateSnapshot) -> Result<Epoch, Box<dyn Error>> {
    Ok(parsed_snapshot.era_history.slot_to_epoch_unchecked_horizon(parsed_snapshot.slot.into())?)
}

pub fn store_nonces(epoch: Epoch, db: &dyn ChainStore, initial_nonces: InitialNonces) -> Result<(), Box<dyn Error>> {
    let header_hash = Hash::from(&initial_nonces.at);

    info!(point.id = %header_hash, point.slot = %initial_nonces.at.slot_or_default(), "importing nonces");

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

        info!(hash = hash.to_string().chars().take(8).collect::<String>(), "inserting header");

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
    info!(count = snapshots.len(), "Importing snapshots");
    for snapshot in snapshots {
        import_snapshot(network, global_parameters, snapshot, ledger_dir).await?;
    }
    info!("Imported snapshots");
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
) -> Result<(), Box<dyn std::error::Error>> {
    import_snapshot_with_optional_nonces(network, global_parameters, snapshot, ledger_dir, None).await?;

    Ok(())
}

async fn import_snapshot_with_optional_nonces(
    network: NetworkName,
    global_parameters: &GlobalParameters,
    snapshot: &Path,
    ledger_dir: &Path,
    nonce_tail: Option<HeaderHash>,
) -> Result<ImportedSnapshot, Box<dyn std::error::Error>> {
    if let Some(paths) = node_snapshot_paths(snapshot) {
        return import_node_snapshot_dir(network, global_parameters, snapshot, &paths, ledger_dir, nonce_tail).await;
    }

    if is_cbor_snapshot_file(snapshot) {
        return import_cbor_snapshot_file(network, global_parameters, snapshot, ledger_dir, nonce_tail).await;
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
) -> Result<ImportedSnapshot, Box<dyn std::error::Error>> {
    info!(snapshot=%snapshot.display(), "Importing CBOR snapshot");

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
    let (db, epoch) = builder
        .spawn(move || {
            import_initial_snapshot(&db, &mut file, &point, &era_history, network, |size, template| {
                TerminalProgressBar::new(size as u64, template).boxed()
            })
            .map_err(|e| e.to_string())
            .map(|epoch| (db, epoch))
        })
        .unwrap()
        .join()
        .unwrap()?;

    db.next_snapshot(epoch)?;

    db.with_transaction(|batch| batch.try_epoch_transition(None, Some(EpochTransitionProgress::SnapshotTaken)))?;

    info!(epoch=%epoch, snapshot=%snapshot.display(), "Imported CBOR snapshot");
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
) -> Result<ImportedSnapshot, Box<dyn std::error::Error>> {
    info!(snapshot=%snapshot_dir.display(), "Importing node snapshot directory");

    std::fs::create_dir_all(ledger_dir)?;

    if std::fs::exists(ledger_dir.join("live"))? {
        std::fs::remove_dir_all(ledger_dir.join("live"))?;
    }

    let db = RocksDB::empty(&RocksDbConfig::new(ledger_dir.to_path_buf()))?;
    let mut state_file = std::fs::File::open(&paths.state)?;
    let mut utxo_file = std::fs::File::open(&paths.utxo)?;

    let global_parameters = global_parameters.clone();
    let builder = std::thread::Builder::new().stack_size(10_000_000);
    let (db, epoch, initial_nonces) = builder
        .spawn(move || {
            import_snapshot_from_tvar(
                &db,
                &mut state_file,
                &mut utxo_file,
                network,
                &global_parameters,
                nonce_tail,
                |size, template| TerminalProgressBar::new(size as u64, template).boxed(),
            )
            .map_err(|e| e.to_string())
            .map(|(epoch, _point, initial_nonces)| (db, epoch, initial_nonces))
        })
        .unwrap()
        .join()
        .unwrap()?;

    db.next_snapshot(epoch)?;

    db.with_transaction(|batch| batch.try_epoch_transition(None, Some(EpochTransitionProgress::SnapshotTaken)))?;

    info!(epoch=%epoch, snapshot=%snapshot_dir.display(), "Imported node snapshot directory");
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

    use amaru_kernel::{Epoch, EraBound, EraHistory, EraName, EraParams, EraSummary, Hasher, HeaderHash, Point, Slot};
    use tempfile::tempdir;

    use super::{
        Snapshot, bootstrap_parent_points, node_snapshot_paths, select_bootstrap_snapshots, snapshot_epoch,
        sort_snapshots_by_slot,
    };
    use crate::cardano_node::ParsedStateSnapshot;

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

    fn snap(epoch: u64) -> Snapshot {
        let slot = 100 * epoch;
        let hash = Hasher::<256>::hash(&slot.to_be_bytes());

        let parent_slot = slot - 1;
        let parent_hash = Hasher::<256>::hash(&parent_slot.to_be_bytes());

        Snapshot {
            epoch: Epoch::from(epoch),
            point: Point::Specific(Slot::from(slot), hash),
            parent_point: Point::Specific(Slot::from(parent_slot), parent_hash),
            url: format!("https://example.com/{slot}.{hash}.tar.gz"),
        }
    }

    #[test]
    fn select_bootstrap_snapshots_defaults_to_latest_epoch_window() {
        assert_eq!(
            select_bootstrap_snapshots(&[snap(165), snap(163), snap(164)], None).unwrap(),
            [&snap(163), &snap(164), &snap(165)]
        )
    }

    #[test]
    fn select_bootstrap_snapshots_honors_requested_start_epoch() {
        assert_eq!(
            select_bootstrap_snapshots(&[snap(165), snap(163), snap(164), snap(166)], Some(Epoch::from(167_u64)))
                .unwrap(),
            [&snap(164), &snap(165), &snap(166)],
        );
    }

    #[test]
    fn select_bootstrap_snapshots_reports_missing_requested_epochs() {
        let err = select_bootstrap_snapshots(&[snap(163), snap(165)], Some(Epoch::from(166_u64))).unwrap_err();
        let err = err.to_string();
        assert!(err.contains("target epoch 166"));
        assert!(err.contains("must contain epochs 163, 164, 165"));
        assert!(err.contains("Available epochs: 163, 165"));
    }

    #[test]
    fn select_bootstrap_snapshots_reports_too_young_epoch() {
        let err = select_bootstrap_snapshots(&[snap(1)], Some(Epoch::from(2_u64))).unwrap_err();
        assert!(dbg!(err.to_string()).contains("target epoch is too young"));
    }

    #[test]
    fn bootstrap_parent_points_use_selected_snapshot_parent_points() {
        assert_eq!(
            bootstrap_parent_points([&snap(163), &snap(164), &snap(165)]),
            vec![snap(164).parent_point, snap(165).parent_point,]
        );
    }
}
