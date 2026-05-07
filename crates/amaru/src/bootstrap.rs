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
    collections::BTreeMap,
    error::Error,
    io,
    path::{Path, PathBuf},
    time::Duration,
};

use amaru_kernel::{BlockHeader, Epoch, Hash, HeaderHash, IsHeader, NetworkName, Nonce, Peer, Point, from_cbor};
use amaru_ledger::store::{EpochTransitionProgress, Store, TransactionalContext};
use amaru_network::chain_sync_client::ChainSyncClient;
use amaru_ouroboros::{ChainStore, Nonces};
use amaru_progress_bar::new_terminal_progress_bar;
use amaru_stores::rocksdb::{RocksDB, RocksDbConfig, consensus::RocksDBStore};
use flate2::read::GzDecoder;
use futures_util::TryStreamExt;
use pallas_network::{facades::PeerClient, miniprotocols::chainsync::NextResponse};
use reqwest::StatusCode;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tar::Archive;
use tokio::{
    fs::{self, File},
    io::AsyncWriteExt,
    time::timeout,
};
use tracing::{error, info};

use crate::{
    cardano_node::{ParsedStateSnapshot, parse_state_snapshot_with_nonces, tvar::import_snapshot_from_tvar},
    default_snapshots_dir, get_bootstrap_file,
};

/// Configuration for a single ledger state's snapshot to be imported.
#[derive(Debug, Deserialize, Clone)]
struct Snapshot {
    /// The snapshot's epoch.
    epoch: Epoch,

    /// The snapshot's point, in the form `<slot>.<header hash>`.
    ///
    /// TODO: make it a genuine `Point` type.
    point: String,

    /// The URL to retrieve snapshot from.
    url: String,

    #[serde(default, alias = "header_parent")]
    parent_point: Option<String>,
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
    MissingParentPoint(String),

    #[error("{0}")]
    SnapshotSelection(String),
}

pub const BOOTSTRAP_HEADERS_PER_POINT: usize = 2;

fn snapshot_directory_path(snapshots_dir: &Path, snapshot: &Snapshot) -> PathBuf {
    snapshots_dir.join(&snapshot.point)
}

fn resolve_snapshot_path(snapshots_dir: &Path, snapshot: &Snapshot) -> Option<PathBuf> {
    let directory = snapshot_directory_path(snapshots_dir, snapshot);
    node_snapshot_paths(&directory).map(|_| directory)
}

fn snapshot_hash(snapshot: &Snapshot) -> Result<HeaderHash, Box<dyn Error>> {
    match Point::try_from(snapshot.point.as_str())? {
        Point::Specific(_, hash) => Ok(hash),
        Point::Origin => Err("bootstrap snapshots must not use origin".into()),
    }
}

fn bootstrap_snapshots(network: NetworkName) -> Result<(PathBuf, Vec<Snapshot>), Box<dyn Error>> {
    let snapshot_file_name = "snapshots.json";
    let snapshots_dir: PathBuf = default_snapshots_dir(network).into();
    let snapshots_file = get_bootstrap_file(network, snapshot_file_name)?
        .ok_or(BootstrapError::MissingConfigFile(snapshot_file_name.into()))?;
    let snapshots: Vec<Snapshot> = serde_json::from_slice(&snapshots_file)
        .map_err(|source| BootstrapError::MalformedSnapshotsFile { path: snapshot_file_name.into(), source })?;

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
    requested_first_epoch: Option<Epoch>,
) -> Result<[&Snapshot; 3], Box<dyn Error>> {
    let snapshots_by_epoch = snapshots.iter().map(|snapshot| (snapshot.epoch, snapshot)).collect::<BTreeMap<_, _>>();
    let latest_epoch = snapshots_by_epoch.keys().next_back().copied().ok_or_else(|| {
        BootstrapError::SnapshotSelection("No bootstrap snapshots are configured in snapshots.json".to_string())
    })?;
    let first_epoch = requested_first_epoch.unwrap_or_else(|| latest_epoch.saturating_sub(2));
    let required_epochs = [first_epoch, first_epoch + 1, first_epoch + 2];

    match required_epochs.map(|epoch| snapshots_by_epoch.get(&epoch).copied()) {
        [Some(first_snapshot), Some(second_snapshot), Some(third_snapshot)] => {
            Ok([first_snapshot, second_snapshot, third_snapshot])
        }
        _ => {
            let available_epochs = snapshots_by_epoch.keys().copied().collect::<Vec<_>>();
            let available_epochs = format_epoch_list(&available_epochs);
            let required_epochs = format_epoch_list(&required_epochs);
            let message = match requested_first_epoch {
                Some(requested_first_epoch) => format!(
                    "bootstrap requested epoch {requested_first_epoch}, but snapshots.json must contain epochs {required_epochs}. Available epochs: {available_epochs}"
                ),
                None => format!(
                    "bootstrap needs the latest 3 consecutive snapshot epochs ending at {latest_epoch}, but snapshots.json only provides epochs {available_epochs}. Required epochs: {required_epochs}"
                ),
            };

            Err(BootstrapError::SnapshotSelection(message).into())
        }
    }
}

fn initial_nonces_from_snapshot(
    snapshot_dir: &Path,
    network: NetworkName,
    tail: HeaderHash,
) -> Result<(Epoch, InitialNonces), Box<dyn Error>> {
    let snapshot_paths = node_snapshot_paths(snapshot_dir)
        .ok_or_else(|| BootstrapError::MissingSnapshotDirectory(snapshot_dir.into()))?;
    let bytes = std::fs::read(snapshot_paths.state)?;

    let (parsed_snapshot, initial_nonces) =
        parse_state_snapshot_with_nonces(minicbor::Decoder::new(&bytes), &network, tail)?;
    let epoch = snapshot_epoch(&parsed_snapshot)?;

    Ok((epoch, initial_nonces))
}

fn default_bootstrap_nonces_from_snapshots(
    snapshots_dir: &Path,
    snapshots: &[Snapshot],
    network: NetworkName,
) -> Result<(Epoch, InitialNonces), Box<dyn Error>> {
    let [_, second_snapshot, third_snapshot] = select_bootstrap_snapshots(snapshots, None)?;
    let third_snapshot_path = resolve_snapshot_path(snapshots_dir, third_snapshot).ok_or_else(|| {
        BootstrapError::MissingSnapshotDirectory(snapshot_directory_path(snapshots_dir, third_snapshot))
    })?;
    let (epoch, initial_nonces) =
        initial_nonces_from_snapshot(&third_snapshot_path, network, snapshot_hash(second_snapshot)?)?;

    Ok((epoch, initial_nonces))
}

pub fn default_bootstrap_nonces(network: NetworkName) -> Result<(Epoch, InitialNonces), Box<dyn Error>> {
    let (snapshots_dir, snapshots) = bootstrap_snapshots(network)?;
    default_bootstrap_nonces_from_snapshots(&snapshots_dir, &snapshots, network)
}

fn snapshot_parent_point(snapshot: &Snapshot) -> Result<Point, Box<dyn Error>> {
    let parent_point =
        snapshot.parent_point.as_deref().ok_or_else(|| BootstrapError::MissingParentPoint(snapshot.point.clone()))?;

    Ok(Point::try_from(parent_point)?)
}

fn bootstrap_parent_points(snapshots: [&Snapshot; 3]) -> Result<Vec<Point>, Box<dyn Error>> {
    let [_, second_snapshot, third_snapshot] = snapshots;
    [second_snapshot, third_snapshot].into_iter().map(snapshot_parent_point).collect()
}

pub fn default_bootstrap_parent_points(network: NetworkName) -> Result<Vec<Point>, Box<dyn Error>> {
    let (_, snapshots) = bootstrap_snapshots(network)?;
    bootstrap_parent_points(select_bootstrap_snapshots(&snapshots, None)?)
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

fn should_download_snapshot(snapshots_dir: &Path, snapshot: &Snapshot) -> bool {
    resolve_snapshot_path(snapshots_dir, snapshot).is_none()
}

async fn download_snapshots(snapshots: &[&Snapshot], snapshots_dir: &Path) -> Result<(), BootstrapError> {
    fs::create_dir_all(snapshots_dir)
        .await
        .map_err(|err| BootstrapError::CreateSnapshotsDir(snapshots_dir.to_path_buf(), err))?;

    let client = reqwest::Client::new();

    for snapshot in snapshots {
        let snapshot_dir = snapshot_directory_path(snapshots_dir, snapshot);
        if !should_download_snapshot(snapshots_dir, snapshot) {
            info!(snapshot = %snapshot_dir.display(), "snapshot directory already exists, skipping download");
            continue;
        }

        if snapshot_dir.exists() {
            info!(snapshot = %snapshot_dir.display(), "snapshot directory exists but is not a valid tvar snapshot, removing it");
            fs::remove_dir_all(&snapshot_dir).await?;
        }

        info!(epoch = %snapshot.epoch, point = %snapshot.point, "downloading snapshot");

        let archive_path = snapshots_dir.join(format!("{}.download.partial", snapshot.point));
        let extract_path = snapshots_dir.join(format!(".{}.extract.partial", snapshot.point));

        let response = client
            .get(&snapshot.url)
            .send()
            .await
            .map_err(|err| BootstrapError::DownloadError(snapshot.url.clone(), err))?;

        if !response.status().is_success() {
            return Err(BootstrapError::DownloadInvalidStatusCode(snapshot.url.clone(), response.status()));
        }

        let mut file = File::create(&archive_path).await?;
        download_to_file(&mut file, response).await?;
        file.sync_all().await?;
        drop(file);

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

async fn download_to_file(file: &mut File, response: reqwest::Response) -> Result<(), BootstrapError> {
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.try_next().await.map_err(io::Error::other)? {
        file.write_all(&chunk).await?;
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
    ledger_dir: PathBuf,
    chain_dir: PathBuf,
    peer_address: &str,
    requested_first_epoch: Option<Epoch>,
) -> Result<(), Box<dyn Error>> {
    let (snapshots_dir, snapshots) = bootstrap_snapshots(network)?;
    let [first_snapshot, second_snapshot, third_snapshot] =
        select_bootstrap_snapshots(&snapshots, requested_first_epoch)?;

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

    import_snapshot(network, &first_snapshot_path, &ledger_dir).await?;
    import_snapshot(network, &second_snapshot_path, &ledger_dir).await?;
    let imported_third_snapshot = import_snapshot_with_optional_nonces(
        network,
        &third_snapshot_path,
        &ledger_dir,
        Some(snapshot_hash(second_snapshot)?),
    )
    .await?;

    let chain_db = RocksDBStore::open_and_migrate(&RocksDbConfig::new(chain_dir.clone()))?;
    let initial_nonces =
        imported_third_snapshot.initial_nonces.ok_or("bootstrap import must produce nonces for the latest snapshot")?;
    store_nonces(imported_third_snapshot.epoch, &chain_db, initial_nonces)?;
    let parent_points = bootstrap_parent_points([first_snapshot, second_snapshot, third_snapshot])?;
    let headers = fetch_headers_from_points(peer_address, network, &parent_points, BOOTSTRAP_HEADERS_PER_POINT).await?;
    import_headers(&chain_db, headers).await?;

    Ok(())
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

fn snapshot_epoch(parsed_snapshot: &ParsedStateSnapshot) -> Result<Epoch, Box<dyn Error>> {
    Ok(parsed_snapshot.era_history.slot_to_epoch_unchecked_horizon(parsed_snapshot.slot.into())?)
}

pub fn store_nonces(
    epoch: Epoch,
    db: &dyn ChainStore<BlockHeader>,
    initial_nonces: InitialNonces,
) -> Result<(), Box<dyn Error>> {
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

#[allow(clippy::unwrap_used)]
pub async fn import_headers(db: &RocksDBStore, headers: Vec<Vec<u8>>) -> Result<(), Box<dyn Error>> {
    for header in headers {
        let block_header: BlockHeader = from_cbor(&header).unwrap();
        let hash = block_header.hash();

        info!(hash = hash.to_string().chars().take(8).collect::<String>(), "inserting header");

        db.store_header(&block_header)?;
    }

    Ok(())
}

pub async fn import_snapshots_from_directory(
    network: NetworkName,
    ledger_dir: &Path,
    snapshot_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if node_snapshot_paths(snapshot_dir).is_some() {
        let snapshots = [snapshot_dir.to_path_buf()];
        return import_snapshots(network, &snapshots, ledger_dir).await;
    }

    let mut snapshots = std::fs::read_dir(snapshot_dir)?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| node_snapshot_paths(path).is_some())
        .collect::<Vec<_>>();

    sort_snapshots_by_slot(&mut snapshots);

    import_snapshots(network, &snapshots, ledger_dir).await
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
    snapshots: &[PathBuf],
    ledger_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    info!(count = snapshots.len(), "Importing snapshots");
    for snapshot in snapshots {
        import_snapshot(network, snapshot, ledger_dir).await?;
    }
    info!("Imported snapshots");
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("expected cardano-node InMem snapshot directory with `state` and `tables/tvar`: {0}")]
    UnsupportedSnapshotPath(PathBuf),
}

struct ImportedSnapshot {
    epoch: Epoch,
    initial_nonces: Option<InitialNonces>,
}

async fn import_snapshot(
    network: NetworkName,
    snapshot: &Path,
    ledger_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    import_snapshot_with_optional_nonces(network, snapshot, ledger_dir, None).await?;

    Ok(())
}

async fn import_snapshot_with_optional_nonces(
    network: NetworkName,
    snapshot: &Path,
    ledger_dir: &Path,
    nonce_tail: Option<HeaderHash>,
) -> Result<ImportedSnapshot, Box<dyn std::error::Error>> {
    if let Some(paths) = node_snapshot_paths(snapshot) {
        return import_node_snapshot_dir(network, snapshot, &paths, ledger_dir, nonce_tail).await;
    }

    Err(Box::new(ImportError::UnsupportedSnapshotPath(snapshot.to_path_buf())))
}

#[expect(clippy::unwrap_used)]
async fn import_node_snapshot_dir(
    network: NetworkName,
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

    let builder = std::thread::Builder::new().stack_size(10_000_000);
    let (db, epoch, initial_nonces) = builder
        .spawn(move || {
            import_snapshot_from_tvar(
                &db,
                &mut state_file,
                &mut utxo_file,
                network,
                nonce_tail,
                new_terminal_progress_bar,
            )
            .map_err(|e| e.to_string())
            .map(|(epoch, _point, initial_nonces)| (db, epoch, initial_nonces))
        })
        .unwrap()
        .join()
        .unwrap()?;

    db.next_snapshot(epoch)?;

    let transaction = db.create_transaction();
    transaction.try_epoch_transition(None, Some(EpochTransitionProgress::SnapshotTaken))?;
    transaction.commit()?;

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

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, time::Duration};

    use amaru_kernel::{Epoch, EraBound, EraHistory, EraName, EraParams, EraSummary, HeaderHash, Point, Slot};
    use tempfile::tempdir;

    use super::{
        Snapshot, bootstrap_parent_points, node_snapshot_paths, select_bootstrap_snapshots, should_download_snapshot,
        snapshot_epoch, sort_snapshots_by_slot,
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

    #[test]
    fn should_download_snapshot_for_invalid_existing_directory() {
        let temp_dir = tempdir().unwrap();
        let snapshots_dir = temp_dir.path();
        let snapshot = Snapshot {
            epoch: Epoch::from(163_u64),
            point: "69206375.hash".to_string(),
            url: "https://example.com/69206375.hash.tar.gz".to_string(),
            parent_point: None,
        };
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
        let snapshot = Snapshot {
            epoch: Epoch::from(163_u64),
            point: "69206375.hash".to_string(),
            url: "https://example.com/69206375.hash.tar.gz".to_string(),
            parent_point: None,
        };
        let snapshot_dir = snapshots_dir.join(&snapshot.point);
        std::fs::create_dir_all(snapshot_dir.join("tables")).unwrap();
        std::fs::write(snapshot_dir.join("state"), b"state").unwrap();
        std::fs::write(snapshot_dir.join("tables").join("tvar"), b"utxo").unwrap();

        assert!(!should_download_snapshot(snapshots_dir, &snapshot));
    }

    #[test]
    fn select_bootstrap_snapshots_defaults_to_latest_epoch_window() {
        let snapshots = vec![
            Snapshot {
                epoch: Epoch::from(165_u64),
                point: "70070379.hash3".to_string(),
                url: "https://example.com/165.tar.gz".to_string(),
                parent_point: Some(
                    "70070331.076218aa483344e34620d3277542ecc9e7b382ae2407a60e177bc3700548364c".to_string(),
                ),
            },
            Snapshot {
                epoch: Epoch::from(163_u64),
                point: "69206375.hash1".to_string(),
                url: "https://example.com/163.tar.gz".to_string(),
                parent_point: None,
            },
            Snapshot {
                epoch: Epoch::from(164_u64),
                point: "69638382.hash2".to_string(),
                url: "https://example.com/164.tar.gz".to_string(),
                parent_point: Some(
                    "69638365.4ec0f5a78431fdcc594eab7db91aff7dfd91c13cc93e9fbfe70cd15a86fadfb2".to_string(),
                ),
            },
        ];

        let [first_snapshot, second_snapshot, third_snapshot] = select_bootstrap_snapshots(&snapshots, None).unwrap();

        assert_eq!(first_snapshot.epoch, Epoch::from(163_u64));
        assert_eq!(second_snapshot.epoch, Epoch::from(164_u64));
        assert_eq!(third_snapshot.epoch, Epoch::from(165_u64));
    }

    #[test]
    fn select_bootstrap_snapshots_honors_requested_start_epoch() {
        let snapshots = vec![
            Snapshot {
                epoch: Epoch::from(163_u64),
                point: "69206375.hash1".to_string(),
                url: "https://example.com/163.tar.gz".to_string(),
                parent_point: None,
            },
            Snapshot {
                epoch: Epoch::from(164_u64),
                point: "69638382.hash2".to_string(),
                url: "https://example.com/164.tar.gz".to_string(),
                parent_point: Some(
                    "69638365.4ec0f5a78431fdcc594eab7db91aff7dfd91c13cc93e9fbfe70cd15a86fadfb2".to_string(),
                ),
            },
            Snapshot {
                epoch: Epoch::from(165_u64),
                point: "70070379.hash3".to_string(),
                url: "https://example.com/165.tar.gz".to_string(),
                parent_point: Some(
                    "70070331.076218aa483344e34620d3277542ecc9e7b382ae2407a60e177bc3700548364c".to_string(),
                ),
            },
            Snapshot {
                epoch: Epoch::from(166_u64),
                point: "70502379.hash4".to_string(),
                url: "https://example.com/166.tar.gz".to_string(),
                parent_point: Some(
                    "70502331.076218aa483344e34620d3277542ecc9e7b382ae2407a60e177bc3700548364d".to_string(),
                ),
            },
        ];

        let [first_snapshot, second_snapshot, third_snapshot] =
            select_bootstrap_snapshots(&snapshots, Some(Epoch::from(164_u64))).unwrap();

        assert_eq!(first_snapshot.epoch, Epoch::from(164_u64));
        assert_eq!(second_snapshot.epoch, Epoch::from(165_u64));
        assert_eq!(third_snapshot.epoch, Epoch::from(166_u64));
    }

    #[test]
    fn select_bootstrap_snapshots_reports_missing_requested_epochs() {
        let snapshots = vec![
            Snapshot {
                epoch: Epoch::from(163_u64),
                point: "69206375.hash1".to_string(),
                url: "https://example.com/163.tar.gz".to_string(),
                parent_point: None,
            },
            Snapshot {
                epoch: Epoch::from(165_u64),
                point: "70070379.hash3".to_string(),
                url: "https://example.com/165.tar.gz".to_string(),
                parent_point: Some(
                    "70070331.076218aa483344e34620d3277542ecc9e7b382ae2407a60e177bc3700548364c".to_string(),
                ),
            },
        ];

        let err = select_bootstrap_snapshots(&snapshots, Some(Epoch::from(163_u64))).unwrap_err();
        let err = err.to_string();

        assert!(err.contains("requested epoch 163"));
        assert!(err.contains("must contain epochs 163, 164, 165"));
        assert!(err.contains("Available epochs: 163, 165"));
    }

    #[test]
    fn bootstrap_parent_points_use_selected_snapshot_parent_points() {
        let snapshots = [
            Snapshot {
                epoch: Epoch::from(163_u64),
                point: "69206375.hash1".to_string(),
                url: "https://example.com/163.tar.gz".to_string(),
                parent_point: None,
            },
            Snapshot {
                epoch: Epoch::from(164_u64),
                point: "69638382.hash2".to_string(),
                url: "https://example.com/164.tar.gz".to_string(),
                parent_point: Some(
                    "69638365.4ec0f5a78431fdcc594eab7db91aff7dfd91c13cc93e9fbfe70cd15a86fadfb2".to_string(),
                ),
            },
            Snapshot {
                epoch: Epoch::from(165_u64),
                point: "70070379.hash3".to_string(),
                url: "https://example.com/165.tar.gz".to_string(),
                parent_point: Some(
                    "70070331.076218aa483344e34620d3277542ecc9e7b382ae2407a60e177bc3700548364c".to_string(),
                ),
            },
        ];

        assert_eq!(
            bootstrap_parent_points([&snapshots[0], &snapshots[1], &snapshots[2]]).unwrap(),
            vec![
                Point::try_from("69638365.4ec0f5a78431fdcc594eab7db91aff7dfd91c13cc93e9fbfe70cd15a86fadfb2").unwrap(),
                Point::try_from("70070331.076218aa483344e34620d3277542ecc9e7b382ae2407a60e177bc3700548364c").unwrap(),
            ]
        );
    }
}
