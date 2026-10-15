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
    collections::BTreeSet,
    fs,
    io::{self, Read},
    path::{Component, Path, PathBuf},
    time::Duration,
};

use amaru_kernel::{
    Credential, Epoch, GlobalParameters, Hash, Header, HeaderHash, IsHeader, NetworkName, NetworkPoint, Point,
    RawBlock, Slot, extract_block_header_cbor, from_cbor,
    num::{CheckedAdd, CheckedSub},
    utils::string::display_collection,
};
use amaru_ledger::store::{EpochTransitionProgress, ReadStore, Store, TransactionalContext};
use amaru_observability::{Instrument, debug_span, error, info};
use amaru_ouroboros::{BaseReadChainStore, ChainStore, Nonces, OpcertSequenceNumbers, WriteChainStore};
use amaru_stores::rocksdb::{ReadOnlyRocksDB, RocksDB, RocksDbConfig, consensus::RocksDBStore};
use anyhow::anyhow;
use pallas_network::{
    facades::PeerClient,
    miniprotocols::{Point as PallasPoint, chainsync::NextResponse},
};
use tar::Archive;
use tokio::{
    fs as async_fs,
    time::{Instant, timeout},
};
pub use tokio_util::sync::CancellationToken as BootstrapCancellation;
use zstd::Decoder as ZstdDecoder;

use crate::{
    aws::{ARCHIVE_EXTENSION, AnonymousS3Client, S3Config},
    cardano_node::tvar::{import_state_from_tvar, import_utxo_from_tvar},
    progress::{
        BootstrapObserver, BootstrapProgress, BootstrapProgressFactory, BootstrapStage, DefaultBootstrapObserver,
    },
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
#[non_exhaustive]
pub enum BootstrapError {
    #[error("expected a snapshot archive in `{}` format: {}", ARCHIVE_EXTENSION, .0.display())]
    UnsupportedSnapshotPath(PathBuf),

    #[error("bootstrap cancelled")]
    Cancelled,

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

    #[error("Invalid bootstrap snapshot point {point}: {reason}")]
    InvalidSnapshotPoint { point: String, reason: String },

    #[error("No bootstrap snapshots found in S3 bucket for this network")]
    NoBootstrapSnapshots,

    #[error("target epoch is too young: {target_epoch}; bootstrap needs at least 3 past epochs")]
    TargetEpochTooYoung { target_epoch: Epoch },

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

    #[error("bootstrap operation failed: {0}")]
    Operation(#[from] anyhow::Error),
}

/// Successful bootstrap summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootstrapReport {
    pub epoch: Epoch,
    pub point: NetworkPoint,
}

pub(crate) fn checkpoint(cancellation: &BootstrapCancellation) -> Result<(), BootstrapError> {
    if cancellation.is_cancelled() { Err(BootstrapError::Cancelled) } else { Ok(()) }
}

pub const BOOTSTRAP_HEADERS_PER_POINT: usize = 2;
const PACKAGED_BLOCKS_FILE_NAME: &str = "bootstrap.blocks.json";
const SNAPSHOT_STATE_FILE_NAME: &str = "state";
const SNAPSHOT_UTXO_FILE_NAME: &str = "tables/tvar";
const SNAPSHOT_SIZE_LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);

fn snapshot_archive_path(snapshots_dir: &Path, snapshot: &Snapshot) -> PathBuf {
    snapshots_dir.join(format!("{}{}", snapshot.point, ARCHIVE_EXTENSION))
}

fn snapshot_hash(snapshot: &Snapshot) -> Result<HeaderHash, BootstrapError> {
    match parse_snapshot_point(&snapshot.point)? {
        NetworkPoint::Specific(_, hash) => Ok(hash),
        NetworkPoint::Origin => Err(BootstrapError::InvalidSnapshotPoint {
            point: snapshot.point.clone(),
            reason: "bootstrap snapshots must not use origin".to_string(),
        }),
    }
}

fn parse_snapshot_point(point: &str) -> Result<NetworkPoint, BootstrapError> {
    NetworkPoint::try_from(point)
        .map_err(|reason| BootstrapError::InvalidSnapshotPoint { point: point.to_string(), reason })
}

/// Discover published snapshots and cached archives, deriving epochs via era history.
async fn bootstrap_snapshots(
    network: NetworkName,
    s3: &AnonymousS3Client,
    snapshots_dir: &Path,
) -> anyhow::Result<Vec<Snapshot>> {
    let mut snapshots = s3.list_snapshots(network).await?;

    match snapshots_dir.read_dir() {
        Ok(entries) => {
            for dir_entry in entries {
                let path = dir_entry?.path();
                let filename = path.file_name().unwrap_or_default().to_str().unwrap_or_default();
                if let Some(prefix) = filename.strip_suffix(ARCHIVE_EXTENSION) {
                    snapshots.push(prefix.to_string());
                }
            }
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            // If there is no directory already created, we should not fail
        }
        Err(err) => return Err(err.into()),
    }

    snapshots_with_epochs(network, snapshots)
}

fn snapshots_with_epochs(network: NetworkName, points: Vec<String>) -> anyhow::Result<Vec<Snapshot>> {
    let era_history = network
        .as_era_history()
        .ok_or_else(|| anyhow!("no era history available for network {network}; S3 bootstrap is only supported for mainnet, preprod, and preview"))?;

    let mut snapshots = Vec::with_capacity(points.len());
    let network_prefix = network.to_string().to_lowercase();
    for point in points {
        let slot = parse_slot_from_point(&point)?;
        let epoch = era_history.slot_to_epoch_unchecked_horizon(Slot::from(slot))?;
        let key = format!("{network_prefix}/{point}{ARCHIVE_EXTENSION}");
        snapshots.push(Snapshot { epoch, point, key });
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

/// List starting epochs supported by three consecutive published snapshots, in ascending order.
///
/// Fetches only the network's public snapshot index, without downloading archives or
/// inspecting local caches. Each returned epoch can be passed to `node bootstrap --epoch`.
/// Returns an empty list when no complete snapshot windows are published.
pub async fn bootstrap_epochs(network: NetworkName, s3_config: S3Config) -> Result<Vec<Epoch>, BootstrapError> {
    let s3 = AnonymousS3Client::new(s3_config);
    let snapshots = snapshots_with_epochs(network, s3.list_snapshots(network).await?)?;
    Ok(available_bootstrap_epochs(&snapshots))
}

/// Input comes from `snapshots_with_epochs`, sorted and deduplicated by epoch.
fn available_bootstrap_epochs(snapshots: &[Snapshot]) -> Vec<Epoch> {
    snapshots
        .windows(3)
        .filter(|window| {
            window[0].epoch.checked_add(Epoch::ONE) == Some(window[1].epoch)
                && window[1].epoch.checked_add(Epoch::ONE) == Some(window[2].epoch)
        })
        .filter_map(|window| window[2].epoch.checked_add(Epoch::ONE))
        .collect()
}

/// Select from the sorted, deduplicated discovery result.
fn select_bootstrap_snapshots(
    snapshots: &[Snapshot],
    target_epoch: Option<Epoch>,
) -> Result<[&Snapshot; 3], BootstrapError> {
    let latest_epoch = snapshots.last().ok_or(BootstrapError::NoBootstrapSnapshots)?.epoch;
    let first_epoch = target_epoch
        .map(|target| {
            target.checked_sub(Epoch::THREE).ok_or(BootstrapError::TargetEpochTooYoung { target_epoch: target })
        })
        .transpose()?
        .unwrap_or_else(|| latest_epoch.saturating_sub(2));
    let required_epochs = [first_epoch, first_epoch + 1, first_epoch + 2];

    match required_epochs.map(|epoch| snapshots.binary_search_by_key(&epoch, |s| s.epoch).ok().map(|i| &snapshots[i])) {
        [Some(first_snapshot), Some(second_snapshot), Some(third_snapshot)] => {
            Ok([first_snapshot, second_snapshot, third_snapshot])
        }
        _ => {
            let available_epochs = available_bootstrap_epochs(snapshots);

            match target_epoch {
                Some(target_epoch) => {
                    Err(BootstrapError::SnapshotSelectionRequestedEpoch { target_epoch, available_epochs })
                }
                None => Err(BootstrapError::SnapshotSelectionLatestEpoch {
                    latest_epoch,
                    required_epochs,
                    available_epochs,
                }),
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
    let mut client = peer_client.chainsync;
    let wire_point = match point {
        NetworkPoint::Origin => PallasPoint::Origin,
        NetworkPoint::Specific(slot, hash) => PallasPoint::Specific(slot.as_u64(), hash.to_vec()),
    };
    let (intersection, _) = client
        .find_intersect(vec![wire_point])
        .instrument(
            debug_span!(
                consensus::chain::FIND_INTERSECTION,
                peer = peer_address,
                intersection_slot = point.slot_or_default()
            )
            .into(),
        )
        .await?;
    let intersection =
        from_pallas_point(&intersection.ok_or_else(|| anyhow!("No intersection found for points: [{point:?}]"))?)?;

    info!(bootstrap::headers::FETCH, requested_point = point, intersection, headers_per_point);

    let mut headers = Vec::with_capacity(headers_per_point);
    while headers.len() < headers_per_point {
        let (operation, next) = if client.has_agency() {
            ("request_next", client.request_next().await)
        } else {
            match timeout(Duration::from_secs(1), client.recv_while_must_reply()).await {
                Ok(next) => ("await_next", next),
                Err(_) => continue,
            }
        };
        let next = next.inspect_err(|error| {
            error!(bootstrap::headers::NEXT_FAILED, operation, error = error.to_string());
        })?;

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
                let point = from_pallas_point(&point)?;
                let tip = from_pallas_point(&tip.0)?.with_height(tip.1.into());
                info!(bootstrap::fetch::ROLLBACK, point, tip);
            }
            NextResponse::Await => continue,
        }
    }

    Ok(headers)
}

fn from_pallas_point(point: &PallasPoint) -> anyhow::Result<NetworkPoint> {
    Ok(match point {
        PallasPoint::Origin => NetworkPoint::Origin,
        PallasPoint::Specific(slot, hash) => {
            NetworkPoint::Specific((*slot).into(), HeaderHash::try_from(hash.as_slice())?)
        }
    })
}

async fn download_snapshots(
    snapshots: &[&Snapshot],
    snapshots_dir: &Path,
    s3: &AnonymousS3Client,
    cancellation: &BootstrapCancellation,
    observer: &dyn BootstrapObserver,
) -> Result<(), BootstrapError> {
    async_fs::create_dir_all(snapshots_dir)
        .await
        .map_err(|err| BootstrapError::CreateSnapshotsDir(snapshots_dir.to_path_buf(), err))?;

    let mut downloaded_bytes = 0_u64;
    let mut completed_snapshots = 0;
    let mut report = |bytes: u64, completed: bool| {
        downloaded_bytes = downloaded_bytes.saturating_add(bytes);
        completed_snapshots += usize::from(completed);
        observer.on_progress(BootstrapProgress::DownloadProgress { downloaded_bytes, completed_snapshots });
    };

    for snapshot in snapshots {
        checkpoint(cancellation)?;
        let archive_path = snapshot_archive_path(snapshots_dir, snapshot);

        if archive_path.is_file() {
            snapshot_archive_root(&archive_path)
                .map_err(|err| BootstrapError::InvalidSnapshotArchive(archive_path.clone(), err.to_string()))?;
            info!(bootstrap::snapshot::SKIP_DOWNLOAD, snapshot = archive_path.display().to_string());
            report(archive_path.metadata()?.len(), true);
            continue;
        }
        if archive_path.exists() {
            return Err(BootstrapError::InvalidSnapshotArchive(
                archive_path,
                format!("snapshot archive path exists but is not a regular `{}` file", ARCHIVE_EXTENSION),
            ));
        }

        let partial_archive_path = snapshots_dir.join(format!(".{}.download.partial", snapshot.point));

        info!(bootstrap::snapshot::DOWNLOAD, epoch = snapshot.epoch, point = snapshot.point);

        tokio::select! {
            biased;
            result = s3.download_object_with_progress(&snapshot.key, &partial_archive_path, |bytes| {
                report(bytes, false);
            }) => {
                result.map_err(|error| BootstrapError::DownloadError(snapshot.key.clone(), error.to_string()))?;
            }
            () = cancellation.cancelled() => return Err(BootstrapError::Cancelled),
        }

        snapshot_archive_root(&partial_archive_path)
            .map_err(|err| BootstrapError::InvalidSnapshotArchive(partial_archive_path.clone(), err.to_string()))?;

        async_fs::rename(partial_archive_path, archive_path).await?;
        report(0, true);
    }

    Ok(())
}

async fn selected_snapshot_size(
    snapshots: &[&Snapshot],
    snapshots_dir: &Path,
    s3: &AnonymousS3Client,
    cancellation: &BootstrapCancellation,
) -> Result<Option<u64>, BootstrapError> {
    let lookups = snapshots.iter().map(|snapshot| async move {
        match snapshot_archive_path(snapshots_dir, snapshot).metadata().ok().filter(|m| m.is_file()).map(|m| m.len()) {
            Some(size) => Some(size),
            None => s3.object_size(&snapshot.key).await.ok().flatten(),
        }
    });
    tokio::select! {
        result = timeout(SNAPSHOT_SIZE_LOOKUP_TIMEOUT, futures_util::future::join_all(lookups)) => {
            Ok(result.ok().and_then(|sizes| sizes.into_iter().try_fold(0_u64, |total, size| total.checked_add(size?))))
        }
        () = cancellation.cancelled() => Err(BootstrapError::Cancelled),
    }
}

fn open_snapshot_archive(path: &Path) -> io::Result<Archive<impl Read>> {
    Ok(Archive::new(ZstdDecoder::new(fs::File::open(path)?)?))
}

fn snapshot_archive_root(archive_path: &Path) -> anyhow::Result<PathBuf> {
    let mut archive = open_snapshot_archive(archive_path)?;
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

pub fn validate_publishable_snapshot_archive(archive_path: &Path, expected_point: &str) -> anyhow::Result<()> {
    let root = snapshot_archive_root(archive_path)?;
    if root != Path::new(expected_point) {
        return Err(anyhow!("snapshot archive root is {}, expected {expected_point}", root.display()));
    }

    load_packaged_block_from_snapshot(archive_path, expected_point).map(|_| ())
}

fn read_snapshot_archive_entry(archive_path: &Path, expected: &Path) -> anyhow::Result<Vec<u8>> {
    let mut archive = open_snapshot_archive(archive_path)?;

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
    path.is_file()
        && path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.ends_with(ARCHIVE_EXTENSION))
}

fn snapshot_archive_entry_matches(path: &Path, expected: &Path) -> bool {
    snapshot_archive_entry_root(path, expected).is_some()
}

fn snapshot_archive_entry_root(path: &Path, expected: &Path) -> Option<PathBuf> {
    let mut components = path.components().filter(|component| *component != Component::CurDir);
    if components.clone().eq(expected.components()) {
        return Some(PathBuf::new());
    }
    let Component::Normal(root) = components.next()? else { return None };
    components.eq(expected.components()).then(|| PathBuf::from(root))
}

/// Bootstrap with cooperative cancellation at decoder and database work-unit boundaries.
///
/// To stop bootstrap, call [`BootstrapCancellation::cancel`] on a cloned handle and continue
/// awaiting this function until it returns [`BootstrapError::Cancelled`]. Do not
/// drop the bootstrap future after requesting cancellation. The cancellation result is returned only
/// after the importer has stopped and released its store handles. During snapshot import,
/// cancellation latency is bounded by the decoder read/sequence chunk or RocksDB batch currently in
/// progress; snapshot downloads are interrupted immediately. Cancellation can leave partial bootstrap
/// stores, which must be removed and bootstrapped again rather than opened as node stores.
/// On success, returns the epoch and point installed in both stores.
#[expect(clippy::too_many_arguments)]
pub async fn bootstrap(
    network: NetworkName,
    global_parameters: &GlobalParameters,
    ledger_dir: PathBuf,
    chain_dir: PathBuf,
    snapshots_dir: PathBuf,
    target_epoch: Option<Epoch>,
    s3_config: S3Config,
    cancellation: BootstrapCancellation,
) -> Result<BootstrapReport, BootstrapError> {
    let observer = DefaultBootstrapObserver::new();
    bootstrap_with_observer(
        network,
        global_parameters,
        ledger_dir,
        chain_dir,
        snapshots_dir,
        target_epoch,
        s3_config,
        cancellation,
        &observer,
    )
    .await
}

/// Bootstrap with canonical progress delivered to `observer`.
///
/// The observer is passive and receives the same state regardless of whether the process has an
/// interactive terminal. [`bootstrap`] installs Amaru's terminal-or-telemetry observer for callers
/// that do not need custom reporting. The returned report contains the same final epoch and point as
/// [`BootstrapProgress::Completed`].
#[expect(clippy::too_many_arguments)]
pub async fn bootstrap_with_observer(
    network: NetworkName,
    global_parameters: &GlobalParameters,
    ledger_dir: PathBuf,
    chain_dir: PathBuf,
    snapshots_dir: PathBuf,
    target_epoch: Option<Epoch>,
    s3_config: S3Config,
    cancellation: BootstrapCancellation,
    observer: &dyn BootstrapObserver,
) -> Result<BootstrapReport, BootstrapError> {
    let cancel_on_drop = cancellation.clone().drop_guard();
    let result = async {
        let started_at = Instant::now();
        checkpoint(&cancellation)?;
        observer.on_progress(BootstrapProgress::StageChanged { stage: BootstrapStage::DiscoveringSnapshots });
        let s3 = AnonymousS3Client::new(s3_config);
        let snapshots = bootstrap_snapshots(network, &s3, &snapshots_dir).await?;
        checkpoint(&cancellation)?;
        observer.on_progress(BootstrapProgress::StageChanged { stage: BootstrapStage::SelectingSnapshots });
        let [first_snapshot, second_snapshot, third_snapshot] = select_bootstrap_snapshots(&snapshots, target_epoch)?;
        let selected_snapshots = [first_snapshot, second_snapshot, third_snapshot];
        let total_bytes = selected_snapshot_size(&selected_snapshots, &snapshots_dir, &s3, &cancellation).await?;
        observer.on_progress(BootstrapProgress::SnapshotsSelected {
            snapshot_count: selected_snapshots.len(),
            total_bytes,
        });

        checkpoint(&cancellation)?;
        observer.on_progress(BootstrapProgress::StageChanged { stage: BootstrapStage::DownloadingSnapshots });
        download_snapshots(&selected_snapshots, &snapshots_dir, &s3, &cancellation, observer).await?;
        checkpoint(&cancellation)?;

        observer.on_progress(BootstrapProgress::StageChanged { stage: BootstrapStage::ImportingSnapshots });
        let mut previous_snapshot = None;
        let mut imported = Vec::with_capacity(selected_snapshots.len());
        for snapshot in selected_snapshots {
            let path = snapshot_archive_path(&snapshots_dir, snapshot);
            if !is_snapshot_archive_file(&path) {
                return Err(BootstrapError::MissingSnapshot(path));
            }
            let nonce_tail = previous_snapshot.map(snapshot_hash).transpose()?;
            let state =
                import_snapshot(network, global_parameters, &path, &ledger_dir, nonce_tail, &cancellation).await?;
            imported.push((path, snapshot, state));
            checkpoint(&cancellation)?;
            previous_snapshot = Some(snapshot);
        }

        observer.on_progress(BootstrapProgress::StageChanged { stage: BootstrapStage::InitializingChainStore });
        let chain_db = RocksDBStore::create(RocksDbConfig::new(chain_dir)).map_err(anyhow::Error::from)?;
        let mut blocks = Vec::with_capacity(2);
        let mut epoch = first_snapshot.epoch;
        // Both packaged tips need nonces before their headers can be stored as validated.
        for (path, snapshot, imported) in imported.into_iter().skip(1) {
            epoch = imported.epoch;
            let chain_state =
                imported.chain_state.ok_or_else(|| anyhow!("bootstrap import must produce chain state"))?;
            store_chain_state(&chain_db, chain_state)?;
            blocks.push(load_packaged_block_from_snapshot(&path, &snapshot.point)?);
        }
        for (header, block) in blocks {
            let hash = header.hash();
            info!(bootstrap::header::IMPORT, header = hash);
            let nonces = chain_db.get_nonces(&hash).ok_or_else(|| {
                anyhow!("bootstrap packaged header {hash} is missing nonces; refuse to store incomplete tip")
            })?;
            chain_db.store_validated_header(&header, &nonces).map_err(anyhow::Error::from)?;
            chain_db.store_block(&hash, &block).map_err(anyhow::Error::from)?;
        }
        checkpoint(&cancellation)?;

        info!(
            bootstrap::COMPLETE,
            duration_seconds = started_at.elapsed().as_secs_f64(),
            epoch,
            point = third_snapshot.point.clone(),
        );
        let report = BootstrapReport { epoch, point: parse_snapshot_point(&third_snapshot.point)? };
        observer.on_progress(BootstrapProgress::Completed { epoch, point: report.point });
        Ok(report)
    }
    .await;
    cancel_on_drop.disarm();
    result
}

fn load_packaged_block_from_snapshot(snapshot_path: &Path, expected_point: &str) -> anyhow::Result<(Header, RawBlock)> {
    if !is_snapshot_archive_file(snapshot_path) {
        return Err(anyhow!("snapshot does not contain packaged bootstrap blocks: {}", snapshot_path.display()));
    }

    let bytes = read_snapshot_archive_entry(snapshot_path, Path::new(PACKAGED_BLOCKS_FILE_NAME))?;
    let hex_blocks: Vec<String> = serde_json::from_slice(&bytes)?;
    let [hex_block]: [String; 1] = hex_blocks.try_into().map_err(|blocks: Vec<String>| {
        anyhow!(
            "packaged bootstrap blocks at {} contain {} blocks; expected exactly 1",
            snapshot_path.display(),
            blocks.len()
        )
    })?;

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

    Ok((header, RawBlock::from(block.into_boxed_slice())))
}

pub(crate) struct ChainState {
    pub point: Point,
    pub nonces: Nonces,
    pub opcert_sequence_numbers: OpcertSequenceNumbers,
}

fn store_chain_state(db: &dyn ChainStore, state: ChainState) -> anyhow::Result<()> {
    info!(bootstrap::nonces::IMPORT, point = state.point);
    db.put_nonces(&Hash::from(&state.point), &state.nonces)?;
    info!(bootstrap::opcert_sequence_numbers::IMPORT, point = state.point);
    db.put_opcert_seed(&state.opcert_sequence_numbers, &state.point)?;
    Ok(())
}

pub async fn import_snapshots(
    network: NetworkName,
    global_parameters: &GlobalParameters,
    snapshots: &[PathBuf],
    ledger_dir: &Path,
) -> anyhow::Result<()> {
    info!(bootstrap::snapshots::IMPORT, count = snapshots.len());
    let cancellation = BootstrapCancellation::new();
    for snapshot in snapshots {
        import_snapshot(network, global_parameters, snapshot, ledger_dir, None, &cancellation).await?;
    }
    Ok(())
}

struct ImportedSnapshot {
    epoch: Epoch,
    chain_state: Option<ChainState>,
}

async fn import_snapshot(
    network: NetworkName,
    global_parameters: &GlobalParameters,
    archive_path: &Path,
    ledger_dir: &Path,
    nonce_tail: Option<HeaderHash>,
    cancellation: &BootstrapCancellation,
) -> Result<ImportedSnapshot, BootstrapError> {
    if !is_snapshot_archive_file(archive_path) {
        return Err(BootstrapError::UnsupportedSnapshotPath(archive_path.to_path_buf()));
    }
    info!(bootstrap::snapshot::IMPORT_ARCHIVE, path = archive_path.display().to_string());
    fs::create_dir_all(ledger_dir)?;

    let previous_accounts = if fs::exists(ledger_dir.join("live"))? {
        let live = ReadOnlyRocksDB::new(&RocksDbConfig::new(ledger_dir.to_path_buf())).map_err(anyhow::Error::from)?;
        let previous_accounts = live
            .iter_accounts()
            .map_err(anyhow::Error::from)?
            .map(|(credential, _)| credential)
            .chain(live.iter_recently_unregistered_accounts().map_err(anyhow::Error::from)?)
            .collect();

        fs::remove_dir_all(ledger_dir.join("live"))?;

        previous_accounts
    } else {
        BTreeSet::new()
    };

    let db = RocksDB::empty(&RocksDbConfig::new(ledger_dir.to_path_buf())).map_err(anyhow::Error::from)?;
    let global_parameters = global_parameters.clone();
    let archive_path = archive_path.to_path_buf();
    let importer_control = cancellation.clone();
    let import_thread = std::thread::Builder::new().stack_size(10_000_000).spawn(move || {
        import_node_snapshot_archive_data(
            &archive_path,
            &db,
            network,
            &global_parameters,
            nonce_tail,
            previous_accounts,
            &importer_control,
        )
        .map(|imported| (db, imported))
    })?;

    let (db, imported) = await_import_thread(import_thread, cancellation).await?;

    db.next_snapshot(imported.epoch).map_err(anyhow::Error::from)?;

    db.with_transaction(|batch| batch.try_epoch_transition(None, Some(EpochTransitionProgress::SnapshotTaken)))
        .map_err(anyhow::Error::from)?;

    Ok(imported)
}

async fn await_import_thread<T: Send + 'static>(
    import_thread: std::thread::JoinHandle<Result<T, BootstrapError>>,
    cancellation: &BootstrapCancellation,
) -> Result<T, BootstrapError> {
    let joined = tokio::task::spawn_blocking(move || import_thread.join())
        .await
        .map_err(|error| BootstrapError::Operation(anyhow!("bootstrap import join task failed: {error}")))?;
    let result = joined.map_err(|_| BootstrapError::Operation(anyhow!("bootstrap import task panicked")))?;
    checkpoint(cancellation)?;
    result
}

fn import_node_snapshot_archive_data(
    archive_path: &Path,
    db: &RocksDB,
    network: NetworkName,
    global_parameters: &GlobalParameters,
    nonce_tail: Option<HeaderHash>,
    previous_accounts: BTreeSet<Credential>,
    cancellation: &BootstrapCancellation,
) -> Result<ImportedSnapshot, BootstrapError> {
    let mut archive = open_snapshot_archive(archive_path)?;

    let mut imported_state = None;
    let mut previous_accounts: Option<BTreeSet<Credential>> = Some(previous_accounts);

    let mut entries = archive.entries()?;
    loop {
        checkpoint(cancellation)?;
        let Some(entry) = entries.next() else {
            break;
        };
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
                cancellation,
            )?);
        } else if snapshot_archive_entry_matches(&path, Path::new(SNAPSHOT_UTXO_FILE_NAME)) {
            let (epoch, point, era_history, chain_state) = imported_state.take().ok_or_else(|| {
                anyhow!(
                    "snapshot archive {} contains {SNAPSHOT_UTXO_FILE_NAME} before {SNAPSHOT_STATE_FILE_NAME}",
                    archive_path.display()
                )
            })?;
            import_utxo_from_tvar(
                &mut entry,
                db,
                &BootstrapProgressFactory,
                &point,
                &era_history,
                network,
                cancellation,
            )?;

            return Ok(ImportedSnapshot { epoch, chain_state });
        }
    }

    Err(BootstrapError::Operation(anyhow!(
        "snapshot archive {} does not contain {SNAPSHOT_UTXO_FILE_NAME}",
        archive_path.display()
    )))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::Cursor,
        path::Path,
        sync::{
            Arc, Barrier,
            atomic::{AtomicBool, Ordering},
        },
    };

    use amaru_kernel::Epoch;
    use tar::{Builder, Header};
    use tempfile::tempdir;

    use super::{
        ARCHIVE_EXTENSION, BootstrapCancellation, BootstrapError, Snapshot, await_import_thread, checkpoint,
        select_bootstrap_snapshots, snapshot_archive_entry_matches, snapshot_archive_root,
        validate_publishable_snapshot_archive,
    };
    fn test_snapshot(epoch: u64, point: &str, network: &str) -> Snapshot {
        Snapshot {
            epoch: Epoch::from(epoch),
            point: point.to_string(),
            key: format!("{network}/{point}{}", ARCHIVE_EXTENSION),
        }
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
    fn cancellation_waits_for_the_import_thread_to_release_its_resources() {
        struct Released(Arc<AtomicBool>);

        impl Drop for Released {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let cancellation = BootstrapCancellation::new();
        let importer_control = cancellation.clone();
        let importer_ready = Arc::new(Barrier::new(2));
        let importer_continue = Arc::new(Barrier::new(2));
        let released = Arc::new(AtomicBool::new(false));
        let import_thread = {
            let importer_ready = Arc::clone(&importer_ready);
            let importer_continue = Arc::clone(&importer_continue);
            let released = Arc::clone(&released);
            std::thread::spawn(move || -> Result<(), BootstrapError> {
                let _released = Released(released);
                importer_ready.wait();
                importer_continue.wait();
                checkpoint(&importer_control)?;
                Ok(())
            })
        };

        importer_ready.wait();
        cancellation.cancel();
        importer_continue.wait();
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let error = runtime.block_on(await_import_thread(import_thread, &cancellation)).unwrap_err();

        assert!(matches!(error, BootstrapError::Cancelled));
        assert!(released.load(Ordering::SeqCst));
    }

    #[test]
    fn rejects_incomplete_or_inconsistent_snapshot_archives() {
        let temp_dir = tempdir().unwrap();
        let archive_path = temp_dir.path().join("69206375.hash.tar.zst");
        for entries in [
            vec![("first/state", b"state".as_slice())],
            vec![("first/state", b"state".as_slice()), ("second/tables/tvar", b"utxo".as_slice())],
        ] {
            write_snapshot_archive(&archive_path, &entries);
            assert!(snapshot_archive_root(&archive_path).is_err());
        }
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
    fn selects_only_complete_three_epoch_windows() {
        let snapshots: Vec<_> =
            [163, 164, 165, 167].into_iter().map(|epoch| test_snapshot(epoch, "point", "preprod")).collect();
        let selected = select_bootstrap_snapshots(&snapshots, Some(Epoch::from(166))).unwrap();
        assert_eq!(selected.map(|s| s.epoch), [163, 164, 165].map(Epoch::from));
        assert!(matches!(
            select_bootstrap_snapshots(&snapshots, None),
            Err(BootstrapError::SnapshotSelectionLatestEpoch { .. })
        ));
        assert!(matches!(
            select_bootstrap_snapshots(&snapshots, Some(Epoch::from(167))),
            Err(BootstrapError::SnapshotSelectionRequestedEpoch { .. })
        ));
        assert!(matches!(
            select_bootstrap_snapshots(&snapshots, Some(Epoch::from(2))),
            Err(BootstrapError::TargetEpochTooYoung { .. })
        ));
        let latest = select_bootstrap_snapshots(&snapshots[..3], None).unwrap();
        assert_eq!(latest.map(|s| s.epoch), selected.map(|s| s.epoch));
    }
}
