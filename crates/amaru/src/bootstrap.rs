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
    error::Error,
    io,
    path::{Path, PathBuf},
};

use amaru_kernel::{BlockHeader, Epoch, Hash, HeaderHash, IsHeader, NetworkName, Nonce, Point, from_cbor};
use amaru_ledger::store::{EpochTransitionProgress, Store, TransactionalContext};
use amaru_ouroboros::{ChainStore, Nonces};
use amaru_progress_bar::new_terminal_progress_bar;
use amaru_stores::rocksdb::{RocksDB, RocksDbConfig, consensus::RocksDBStore};
use async_compression::tokio::bufread::GzipDecoder;
use futures_util::TryStreamExt;
use reqwest::StatusCode;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tokio::{
    fs::{self, File},
    io::BufReader,
};
use tokio_util::io::StreamReader;
use tracing::info;

use crate::{
    cardano_node::{ParsedStateSnapshot, parse_state_snapshot_with_nonces, tvar::import_snapshot_from_tvar},
    default_snapshots_dir, get_bootstrap_file, get_bootstrap_headers,
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
}

#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    #[error("Missing configuration file {0}")]
    MissingConfigFile(PathBuf),

    #[error("Failed to parse snapshots JSON file {0}")]
    MalformedSnapshotsFile(serde_json::Error),

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
}

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
    let snapshots: Vec<Snapshot> =
        serde_json::from_slice(&snapshots_file).map_err(BootstrapError::MalformedSnapshotsFile)?;

    Ok((snapshots_dir, snapshots))
}

fn latest_bootstrap_snapshots(snapshots: &[Snapshot]) -> Result<(&Snapshot, &Snapshot, &Snapshot), Box<dyn Error>> {
    let snapshot_count = snapshots.len();
    if snapshot_count < 3 {
        return Err("Expected at least 3 snapshots in the configuration file".into());
    }

    Ok((&snapshots[snapshot_count - 3], &snapshots[snapshot_count - 2], &snapshots[snapshot_count - 1]))
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
    let (_, second_snapshot, third_snapshot) = latest_bootstrap_snapshots(snapshots)?;
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

fn should_download_snapshot(snapshots_dir: &Path, snapshot: &Snapshot) -> bool {
    !snapshot_directory_path(snapshots_dir, snapshot).is_dir()
}

async fn download_snapshots(snapshots: &[Snapshot], snapshots_dir: &Path) -> Result<(), BootstrapError> {
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

        info!(epoch = %snapshot.epoch, point = %snapshot.point, "downloading snapshot");

        let target_path = snapshots_dir.join(format!("{}.cbor", snapshot.point));

        let response = client
            .get(&snapshot.url)
            .send()
            .await
            .map_err(|err| BootstrapError::DownloadError(snapshot.url.clone(), err))?;

        if !response.status().is_success() {
            return Err(BootstrapError::DownloadInvalidStatusCode(snapshot.url.clone(), response.status()));
        }

        let (tmp_path, file) = uncompress_to_temp_file(&target_path, response).await?;
        file.sync_all().await?;
        fs::rename(tmp_path, &target_path).await?;

        info!(snapshot = %target_path.display(), "downloaded snapshot");
    }

    Ok(())
}

async fn uncompress_to_temp_file(
    target_path: &Path,
    response: reqwest::Response,
) -> Result<(PathBuf, File), BootstrapError> {
    let tmp_path = target_path.with_extension("partial");
    let mut file = File::create(&tmp_path).await?;
    let raw_stream_reader = StreamReader::new(response.bytes_stream().map_err(io::Error::other));
    let buffered_reader = BufReader::new(raw_stream_reader);
    let mut decoded_stream = GzipDecoder::new(buffered_reader);

    tokio::io::copy(&mut decoded_stream, &mut file).await?;

    Ok((tmp_path, file))
}

/// Set the internal dbs in such a state that amaru can run
pub async fn bootstrap(network: NetworkName, ledger_dir: PathBuf, chain_dir: PathBuf) -> Result<(), Box<dyn Error>> {
    let (snapshots_dir, snapshots) = bootstrap_snapshots(network)?;

    download_snapshots(&snapshots, &snapshots_dir).await?;

    let (first_snapshot, second_snapshot, third_snapshot) = latest_bootstrap_snapshots(&snapshots)?;
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
    import_headers(&chain_db, get_bootstrap_headers(network)?.collect::<Vec<_>>()).await?;

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

    use amaru_kernel::{Epoch, EraBound, EraHistory, EraName, EraParams, EraSummary, HeaderHash, Slot};

    use crate::{
        bootstrap::{snapshot_epoch, sort_snapshots_by_slot},
        cardano_node::ParsedStateSnapshot,
    };

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
}
