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

use amaru_consensus::{ChainStore, Nonces};
use amaru_kernel::{
    BlockHeader, EraHistory, Hash, HeaderHash, IsHeader, Nonce, Point, from_cbor,
    network::NetworkName,
};
use amaru_ledger::{
    bootstrap::import_initial_snapshot,
    store::{EpochTransitionProgress, Store, TransactionalContext},
};
use amaru_progress_bar::new_terminal_progress_bar;
use amaru_stores::rocksdb::{RocksDB, RocksDbConfig, consensus::RocksDBStore};
use async_compression::tokio::bufread::GzipDecoder;
use futures_util::TryStreamExt;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{
    error::Error,
    io,
    path::{Path, PathBuf},
};
use tokio::{
    fs::{self, File},
    io::{AsyncReadExt, BufReader},
};
use tokio_util::io::StreamReader;
use tracing::{Level, error, info, instrument, warn};

use crate::snapshots_dir;

/// Configuration for a single ledger state's snapshot to be imported.
#[derive(Debug, Deserialize)]
struct Snapshot {
    /// The snapshot's epoch.
    epoch: u64,

    /// The snapshot's point, in the form `<slot>.<header hash>`.
    ///
    /// TODO: make it a genuine `Point` type.
    point: String,

    /// The URL to retrieve snapshot from.
    url: String,
}

#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    #[error("Can not read Snapshot configuration file {0}: {1}")]
    ReadSnapshotsFile(PathBuf, io::Error),

    #[error("Can not create snapshots directory {0}: {1}")]
    CreateSnapshotsDir(PathBuf, io::Error),

    #[error("Failed to parse snapshots JSON file {0}: {1}")]
    MalformedSnapshotsFile(PathBuf, serde_json::Error),

    #[error("Unable to store snapshots on disk: {0}")]
    Io(#[from] std::io::Error),

    #[error("Failed to download snapshot at url {0}: {1}")]
    DownloadError(String, reqwest::Error),

    #[error("Failed to download snapshot from {0}: HTTP status code {1}")]
    DownloadInvalidStatusCode(String, reqwest::StatusCode),
}

async fn download_snapshots(
    snapshots_file: &PathBuf,
    snapshots_dir: &PathBuf,
) -> Result<(), BootstrapError> {
    // Create the target directory if it doesn't exist
    fs::create_dir_all(snapshots_dir)
        .await
        .map_err(|e| BootstrapError::CreateSnapshotsDir(snapshots_dir.clone(), e))?;

    // Read the snapshots JSON file
    let snapshots_content = fs::read_to_string(snapshots_file)
        .await
        .map_err(|e| BootstrapError::ReadSnapshotsFile(snapshots_file.clone(), e))?;
    let snapshots: Vec<Snapshot> = serde_json::from_str(&snapshots_content)
        .map_err(|e| BootstrapError::MalformedSnapshotsFile(snapshots_file.clone(), e))?;

    // Create a reqwest client
    let client = reqwest::Client::new();

    // Download each snapshot
    for snapshot in &snapshots {
        info!(epoch=%snapshot.epoch, point=%snapshot.point,
            "Downloading snapshot",
        );

        // Extract filename from the point
        let filename = format!("{}.cbor", snapshot.point);
        let target_path = snapshots_dir.join(&filename);

        // Skip if file already exists
        if target_path.exists() {
            info!("Snapshot {} already exists, skipping", filename);
            continue;
        }

        // Download the file
        let response = client
            .get(&snapshot.url)
            .send()
            .await
            .map_err(|e| BootstrapError::DownloadError(snapshot.url.clone(), e))?;
        if !response.status().is_success() {
            return Err(BootstrapError::DownloadInvalidStatusCode(
                snapshot.url.clone(),
                response.status(),
            ));
        }

        let (tmp_path, file) = uncompress_to_temp_file(&target_path, response).await?;

        file.sync_all().await?;
        tokio::fs::rename(&tmp_path, &target_path).await?;

        info!("Downloaded snapshot to {}", target_path.display());
    }

    info!(
        "All {} snapshots downloaded and decompressed successfully",
        snapshots.len()
    );
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
///
/// Idempotent; will do nothing if the dbs have already been boostrapped
/// Returns `true` if bootsrap was actually done
pub async fn bootstrap(
    network: NetworkName,
    ledger_dir: PathBuf,
    chain_dir: PathBuf,
    network_dir: PathBuf,
) -> Result<bool, Box<dyn Error>> {
    let snapshots_file: PathBuf = network_dir.join("snapshots.json");
    let snapshots_dir = PathBuf::from(snapshots_dir(network));
    let config = RocksDbConfig::new(ledger_dir.clone());
    if !config.exists() {
        download_snapshots(&snapshots_file, &snapshots_dir).await?;
        import_snapshots_from_directory(network, &ledger_dir, &snapshots_dir).await?;
        import_nonces_for_network(network, &network_dir, &chain_dir).await?;
        import_headers_for_network(&network_dir, &chain_dir).await?;
        Ok(true)
    } else {
        Ok(false)
    }
}

async fn import_nonces_for_network(
    network: NetworkName,
    config_dir: &Path,
    chain_dir: &PathBuf,
) -> Result<(), Box<dyn Error>> {
    let nonces_file: PathBuf = config_dir.join("nonces.json");
    import_nonces_from_file(network.into(), &nonces_file, chain_dir).await?;
    Ok(())
}

fn deserialize_point<'de, D>(deserializer: D) -> Result<Point, D::Error>
where
    D: Deserializer<'de>,
{
    let buf = <&str>::deserialize(deserializer)?;
    Point::try_from(buf)
        .map_err(|e| serde::de::Error::custom(format!("cannot convert vector to nonce: {:?}", e)))
}

fn serialize_point<S: Serializer>(point: &Point, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&point.to_string())
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InitialNonces {
    #[serde(
        serialize_with = "serialize_point",
        deserialize_with = "deserialize_point"
    )]
    pub at: Point,
    pub active: Nonce,
    pub evolving: Nonce,
    pub candidate: Nonce,
    pub tail: HeaderHash,
}

pub(crate) async fn import_nonces(
    era_history: &EraHistory,
    chain_db_path: &PathBuf,
    initial_nonce: InitialNonces,
) -> Result<(), Box<dyn Error>> {
    let db = Box::new(RocksDBStore::open_and_migrate(RocksDbConfig::new(
        chain_db_path.into(),
    ))?) as Box<dyn ChainStore<BlockHeader>>;

    let header_hash = Hash::from(&initial_nonce.at);

    info!(point.id = %header_hash, point.slot = %initial_nonce.at.slot_or_default(), "importing nonces");

    let epoch = {
        let slot = initial_nonce.at.slot_or_default();
        // NOTE: The slot definitely exists and is within one of the known eras.
        era_history.slot_to_epoch_unchecked_horizon(slot)?
    };

    let nonces = Nonces {
        epoch,
        active: initial_nonce.active,
        evolving: initial_nonce.evolving,
        candidate: initial_nonce.candidate,
        tail: initial_nonce.tail,
    };

    db.put_nonces(&header_hash, &nonces)?;

    Ok(())
}

pub async fn import_nonces_from_file(
    era_history: &EraHistory,
    nonces_file: &PathBuf,
    chain_dir: &PathBuf,
) -> Result<(), Box<dyn Error>> {
    let content = tokio::fs::read_to_string(nonces_file).await?;
    let initial_nonces: InitialNonces = serde_json::from_str(&content)?;
    import_nonces(era_history, chain_dir, initial_nonces).await?;
    Ok(())
}

#[allow(clippy::unwrap_used)]
#[instrument(level = Level::INFO, name = "import_headers")]
pub async fn import_headers_for_network(
    config_dir: &Path,
    chain_dir: &PathBuf,
) -> Result<(), Box<dyn Error>> {
    let db = RocksDBStore::open_and_migrate(RocksDbConfig::new(chain_dir.into()))?;

    for entry in std::fs::read_dir(config_dir.join("headers"))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file()
            && let Some(filename) = path.file_name().and_then(|f| f.to_str())
            && filename.starts_with("header.")
            && filename.ends_with(".cbor")
        {
            let mut file = File::open(&path).await
                .inspect_err(|reason| error!(file = %path.display(), reason = %reason, "Failed to open header file"))?;

            let mut cbor_data = Vec::new();
            file.read_to_end(&mut cbor_data).await
                .inspect_err(|reason| error!(file = %path.display(), reason = %reason, "Failed to read header file"))?;

            let header_from_file: BlockHeader = from_cbor(&cbor_data).unwrap();
            let hash = header_from_file.hash();

            info!(
                hash = hash.to_string().chars().take(8).collect::<String>(),
                "inserting header"
            );

            db.store_header(&header_from_file)?;
        } else {
            warn!(file = %path.display(), "not a header file; ignoring");
        }
    }

    Ok(())
}

pub async fn import_snapshots_from_directory(
    network: NetworkName,
    ledger_dir: &PathBuf,
    snapshot_dir: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut snapshots = std::fs::read_dir(snapshot_dir)?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().and_then(|s| s.to_str()) == Some("cbor"))
        .collect::<Vec<_>>();

    sort_snapshots_by_slot(&mut snapshots);

    import_snapshots(network, &snapshots, ledger_dir).await
}

fn sort_snapshots_by_slot(snapshots: &mut [PathBuf]) {
    // Sort by parsed slot number from filename
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
    snapshots: &Vec<PathBuf>,
    ledger_dir: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("Importing {} snapshots", snapshots.len());
    for snapshot in snapshots {
        import_snapshot(network, snapshot, ledger_dir).await?;
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("malformed date: {}", .0)]
    MalformedDate(String),
    #[error("invalid snapshot file: {0}")]
    InvalidSnapshotFile(PathBuf),
    #[error(
        "You must provide either a single .cbor snapshot file (--snapshot) or a directory containing multiple .cbor snapshots (--snapshot-dir)"
    )]
    IncorrectUsage,
}

#[expect(clippy::unwrap_used)]
pub async fn import_snapshot(
    network: NetworkName,
    snapshot: &PathBuf,
    ledger_dir: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("Importing snapshot {}", snapshot.display());
    let point = Point::try_from(
        snapshot
            .as_path()
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap(),
    )
    .map_err(ImportError::MalformedDate)?;

    std::fs::create_dir_all(ledger_dir)?;

    if std::fs::exists(ledger_dir.join("live"))? {
        std::fs::remove_dir_all(ledger_dir.join("live"))?;
    }

    let db = RocksDB::empty(RocksDbConfig::new(ledger_dir.into()))?;
    let mut file = std::fs::File::open(snapshot)?;
    let dir = snapshot
        .parent()
        .ok_or(ImportError::InvalidSnapshotFile(snapshot.into()))?;

    let era_history = make_era_history(dir, &point, network)?;

    // Increase the stack size slightly as for some reasons, the lazy decoder is greedy on the
    // stack in some situations.
    let builder = std::thread::Builder::new().stack_size(10_000_000);
    let (db, epoch) = builder
        .spawn(move || {
            import_initial_snapshot(
                &db,
                &mut file,
                &point,
                &era_history,
                network,
                new_terminal_progress_bar,
                true,
            )
            .map_err(|e| e.to_string())
            .map(|epoch| (db, epoch))
        })
        .unwrap()
        .join()
        .unwrap()?;

    db.next_snapshot(epoch)?;

    let transaction = db.create_transaction();
    transaction.try_epoch_transition(None, Some(EpochTransitionProgress::SnapshotTaken))?;
    transaction.commit()?;

    info!("Imported snapshot for epoch {}", epoch);
    Ok(())
}

fn make_era_history(
    dir: &Path,
    point: &Point,
    network: NetworkName,
) -> Result<EraHistory, Box<dyn std::error::Error>> {
    match network {
        NetworkName::Testnet(_) => {
            let filename = format!("history.{}.{}.json", point.slot_or_default(), point.hash());
            let history_file = dir.join(filename);
            if !history_file.is_file() {
                return Err(
                    format!("cannot import testnet era history from {:?}", history_file).into(),
                );
            };

            Ok(serde_json::from_slice(&std::fs::read(&history_file)?)?)
        }
        NetworkName::Mainnet | NetworkName::Preprod | NetworkName::Preview => {
            Ok(<&EraHistory>::from(network).clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, str::FromStr};

    use amaru_kernel::{Hash, HeaderHash, Point, Slot, network::NetworkName};
    use amaru_slot_arithmetic::TimeMs;

    use crate::bootstrap::{make_era_history, sort_snapshots_by_slot};

    #[test]
    fn make_era_history_for_tesnet_given_file_exists() {
        let dir = PathBuf::from("tests/data/");
        let hash: HeaderHash =
            Hash::from_str("4df4505d862586f9e2c533c5fbb659f04402664db1b095aba969728abfb77301")
                .unwrap();
        let point = Point::Specific(56073562, hash.to_vec());

        let history = make_era_history(&dir, &point, NetworkName::Testnet(14))
            .expect("fail to make era history");

        assert_eq!(
            TimeMs::from(5100000000),
            history
                .slot_to_relative_time_unchecked_horizon(Slot::from(5100000))
                .unwrap()
        );
    }

    #[test]
    fn sort_snapshot_file_names_by_slot_number() {
        let mut paths = [
            PathBuf::from(
                "172786.932b9688167139cf4792e97ae4771b6dc762ad25752908cce7b24c2917847516.cbor",
            ),
            PathBuf::from(
                "259174.a07da7616822a1ccb4811e907b1f3a3c5274365908a241f4d5ffab2a69eb8802.cbor",
            ),
            PathBuf::from(
                "86392.1d38de4ffae6090c24151578d331b1021adb8f37d158011616db4d47d1704968.cbor",
            ),
        ];

        sort_snapshots_by_slot(&mut paths);

        assert_eq!(
            PathBuf::from(
                "86392.1d38de4ffae6090c24151578d331b1021adb8f37d158011616db4d47d1704968.cbor"
            ),
            paths[0]
        );
    }
}
