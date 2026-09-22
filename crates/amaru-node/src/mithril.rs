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
    ffi::OsString,
    fs::{self, File, OpenOptions, TryLockError},
    future::Future,
    io::{self, IsTerminal},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use amaru_consensus::{block_validator::BlockValidator, validate_header::validate_header};
use amaru_kernel::{
    Block, ConsensusParameters, EraHistory, IsHeader, NetworkName, NetworkPoint, ORIGIN_HASH, Point, RawBlock, Slot,
    cardano::network_block::NetworkBlock,
};
use amaru_ledger::store::ReadStore;
use amaru_mithril::{
    MithrilDownloadError, MithrilDownloadObserver, MithrilDownloadProgress, MithrilDownloadReport,
    download_from_mithril_for_range_with_observer, read_blocks_after_point,
};
use amaru_observability::info;
use amaru_ouroboros::{ChainStore, PoolSummaries, can_validate_blocks::CanValidateBlocks};
use amaru_progress_bar::{ProgressBar, TerminalProgressBar};
use amaru_stores::rocksdb::{ReadOnlyRocksDB, RocksDbConfig, consensus::RocksDBStore};
use anyhow::anyhow;
use thiserror::Error;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::{
    ClearValidity, realign_chain_store_to,
    stages::{
        build_node::{make_block_validator, make_state},
        config::LedgerConfig,
    },
};

const LEDGER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
const STRUCTURED_PROGRESS_INTERVAL: Duration = Duration::from_secs(5);

/// Cooperative cancellation for [`MithrilSynchronizer::synchronize`].
pub type MithrilCancellation = CancellationToken;

/// A high-level stage of Mithril synchronization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MithrilStage {
    ResolvingResumePoint,
    Downloading,
    RecoveringStores,
    Ingesting,
}

impl MithrilStage {
    fn as_str(self) -> &'static str {
        match self {
            Self::ResolvingResumePoint => "resolving_resume_point",
            Self::Downloading => "downloading",
            Self::RecoveringStores => "recovering_stores",
            Self::Ingesting => "ingesting",
        }
    }
}

/// Successful synchronization summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MithrilSyncReport {
    pub resume_point: Point,
    pub final_point: Point,
    /// Selected snapshot, or `None` when the existing immutable cache made a new snapshot unnecessary.
    pub snapshot_hash: Option<String>,
    pub processed_blocks: u64,
}

/// Canonical progress shared by terminal, structured, and custom observers.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MithrilProgress {
    StageChanged { stage: MithrilStage },
    SnapshotSelected { hash: String, through_chunk: u64 },
    Downloaded { downloaded_bytes: u64, completed_files: u64, total_files: u64, total_bytes: Option<u64> },
    BlocksIngested { blocks: u64, point: Point },
    Completed { report: MithrilSyncReport },
}

/// Passive consumer of canonical synchronization progress.
pub trait MithrilObserver: Send + Sync {
    fn on_progress(&self, progress: MithrilProgress);
}

/// Default observer used by the CLI.
pub struct DefaultMithrilObserver {
    renderer: Mutex<DefaultRenderer>,
}

impl DefaultMithrilObserver {
    pub fn new() -> Self {
        let renderer = if io::stderr().is_terminal() {
            DefaultRenderer::Terminal(TerminalRenderer::default())
        } else {
            DefaultRenderer::Structured(StructuredRenderer::default())
        };
        Self { renderer: Mutex::new(renderer) }
    }
}

impl Default for DefaultMithrilObserver {
    fn default() -> Self {
        Self::new()
    }
}

impl MithrilObserver for DefaultMithrilObserver {
    fn on_progress(&self, progress: MithrilProgress) {
        self.renderer.lock().unwrap_or_else(std::sync::PoisonError::into_inner).render(progress);
    }
}

enum DefaultRenderer {
    Terminal(TerminalRenderer),
    Structured(StructuredRenderer),
}

impl DefaultRenderer {
    fn render(&mut self, progress: MithrilProgress) {
        match self {
            Self::Terminal(renderer) => renderer.render(progress),
            Self::Structured(renderer) => renderer.render(progress),
        }
    }
}

#[derive(Default)]
struct StructuredRenderer {
    last_download_at: Option<Instant>,
    last_ingest_at: Option<Instant>,
    completed_files: u64,
}

impl StructuredRenderer {
    fn render(&mut self, progress: MithrilProgress) {
        if self.should_render(&progress, Instant::now()) {
            render_structured(progress);
        }
    }

    fn should_render(&mut self, progress: &MithrilProgress, now: Instant) -> bool {
        match progress {
            MithrilProgress::Downloaded { completed_files, .. } => {
                let completed = *completed_files > self.completed_files;
                let interval_elapsed = self.last_download_at.is_none_or(|last_download_at| {
                    now.duration_since(last_download_at) >= STRUCTURED_PROGRESS_INTERVAL
                });
                self.completed_files = *completed_files;
                if completed || interval_elapsed {
                    self.last_download_at = Some(now);
                    true
                } else {
                    false
                }
            }
            MithrilProgress::BlocksIngested { .. } => {
                let interval_elapsed = self
                    .last_ingest_at
                    .is_none_or(|last_ingest_at| now.duration_since(last_ingest_at) >= STRUCTURED_PROGRESS_INTERVAL);
                if interval_elapsed {
                    self.last_ingest_at = Some(now);
                }
                interval_elapsed
            }
            MithrilProgress::StageChanged { .. }
            | MithrilProgress::SnapshotSelected { .. }
            | MithrilProgress::Completed { .. } => true,
        }
    }
}

#[derive(Default)]
struct TerminalRenderer {
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    download: Option<Box<dyn ProgressBar>>,
}

impl TerminalRenderer {
    fn render(&mut self, progress: MithrilProgress) {
        match progress {
            MithrilProgress::Downloaded { downloaded_bytes, total_bytes, .. } => {
                let total_bytes = total_bytes.filter(|total_bytes| *total_bytes > 0);
                let total_became_known = self.total_bytes.is_none() && total_bytes.is_some();
                if total_became_known && let Some(download) = self.download.take() {
                    download.clear();
                }
                let download = self.download.get_or_insert_with(|| {
                    let (length, template) = download_progress_bar_spec(total_bytes);
                    TerminalProgressBar::new(length, template).boxed()
                });
                let increment = if total_became_known {
                    downloaded_bytes
                } else {
                    downloaded_bytes.saturating_sub(self.downloaded_bytes)
                };
                download.tick(usize::try_from(increment).unwrap_or(usize::MAX));
                self.downloaded_bytes = downloaded_bytes;
                self.total_bytes = total_bytes;
            }
            MithrilProgress::StageChanged { stage: MithrilStage::Ingesting } | MithrilProgress::Completed { .. } => {
                if let Some(download) = self.download.take() {
                    download.finish();
                }
            }
            MithrilProgress::StageChanged { .. }
            | MithrilProgress::SnapshotSelected { .. }
            | MithrilProgress::BlocksIngested { .. } => {}
        }
    }
}

fn download_progress_bar_spec(total_bytes: Option<u64>) -> (u64, &'static str) {
    match total_bytes.filter(|total_bytes| *total_bytes > 0) {
        Some(total_bytes) => (
            total_bytes,
            "{spinner:.green} Downloading Mithril files {bytes_per_sec:>10} {bar:40.green} [{bytes:>10}/{total_bytes:<10}] ({eta} remaining)",
        ),
        None => (0, "{spinner:.green} Downloading Mithril files {bytes_per_sec:>10} [{bytes:>10} downloaded]"),
    }
}

fn render_structured(progress: MithrilProgress) {
    match progress {
        MithrilProgress::StageChanged { stage } => {
            info!(mithril::progress::STAGE, stage = stage.as_str().to_owned());
        }
        MithrilProgress::SnapshotSelected { hash, through_chunk } => {
            info!(mithril::progress::SNAPSHOT, hash, through_chunk);
        }
        MithrilProgress::Downloaded { downloaded_bytes, completed_files, total_files, total_bytes } => {
            info!(
                mithril::progress::DOWNLOAD,
                downloaded_bytes,
                completed_files,
                total_files,
                total_bytes = @total_bytes
            );
        }
        MithrilProgress::BlocksIngested { blocks, point } => {
            info!(mithril::progress::INGEST, blocks, point);
        }
        MithrilProgress::Completed { report } => {
            info!(mithril::progress::COMPLETE, point = report.final_point, processed_blocks = report.processed_blocks);
        }
    }
}

/// Store state that cannot be reconciled without rebuilding from a trusted snapshot.
#[derive(Debug, Error)]
#[error("{reason} (ledger {ledger_tip}, chain {chain_tip})")]
pub struct RebootstrapRequired {
    pub ledger_tip: Point,
    pub chain_tip: Point,
    pub reason: String,
}

/// Typed synchronization failures.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum MithrilSyncError {
    #[error("Mithril synchronization was cancelled")]
    Cancelled,
    #[error("another Mithril synchronization is using {path}")]
    Concurrent { path: PathBuf },
    #[error("resume point {requested} does not match ledger tip {stored}")]
    ResumePointMismatch { requested: NetworkPoint, stored: NetworkPoint },
    #[error("cannot resolve resume point {point} in the chain store")]
    ResumePointNotFound { point: NetworkPoint },
    #[error("no applicable Mithril snapshot is available")]
    SnapshotUnavailable {
        #[source]
        source: anyhow::Error,
    },
    #[error("latest Mithril snapshot ends at chunk {through_chunk}, before required chunk {required_chunk}")]
    SnapshotInapplicable { through_chunk: u64, required_chunk: u64 },
    #[error("Mithril snapshot download failed")]
    Download {
        #[source]
        source: anyhow::Error,
    },
    #[error("Mithril snapshot validation failed")]
    SnapshotValidation {
        #[source]
        source: anyhow::Error,
    },
    #[error("the local Mithril cache is invalid")]
    InvalidCache {
        #[source]
        source: anyhow::Error,
    },
    #[error("block validation failed at {point}")]
    Validation {
        point: Point,
        #[source]
        source: anyhow::Error,
    },
    #[error("store operation failed: {operation}")]
    Store {
        operation: &'static str,
        #[source]
        source: anyhow::Error,
    },
    #[error("interrupted store mutation cannot be recovered: {0}")]
    RebootstrapRequired(Box<RebootstrapRequired>),
    #[error("failed to stop the ledger worker")]
    WorkerShutdown {
        #[source]
        source: anyhow::Error,
    },
}

/// End-to-end Mithril synchronization configuration.
///
/// Custom observers receive the same canonical events as the CLI renderer:
///
/// ```ignore
/// use std::sync::Arc;
///
/// use amaru_node::{MithrilCancellation, MithrilObserver, MithrilProgress, MithrilSynchronizer, NetworkName};
///
/// struct Observer;
///
/// impl MithrilObserver for Observer {
///     fn on_progress(&self, progress: MithrilProgress) {
///         if let MithrilProgress::BlocksIngested { blocks, point } = progress {
///             println!("ingested {blocks} blocks through {point}");
///         }
///     }
/// }
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let synchronizer = MithrilSynchronizer::new(
///     NetworkName::Preprod,
///     "ledger.preprod.db",
///     "chain.preprod.db",
///     "mithril-snapshots",
/// );
/// synchronizer.synchronize(MithrilCancellation::new(), Arc::new(Observer)).await?;
/// # Ok(())
/// # }
/// ```
pub struct MithrilSynchronizer {
    network: NetworkName,
    ledger_dir: PathBuf,
    chain_dir: PathBuf,
    snapshots_dir: PathBuf,
    resume_point: Option<NetworkPoint>,
    ingest_until_slot: Option<Slot>,
    ingest_maximum_blocks: Option<usize>,
}

impl MithrilSynchronizer {
    pub fn new(
        network: NetworkName,
        ledger_dir: impl Into<PathBuf>,
        chain_dir: impl Into<PathBuf>,
        snapshots_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            network,
            ledger_dir: ledger_dir.into(),
            chain_dir: chain_dir.into(),
            snapshots_dir: snapshots_dir.into(),
            resume_point: None,
            ingest_until_slot: None,
            ingest_maximum_blocks: None,
        }
    }

    /// Require synchronization to start from this ledger identity.
    pub fn resume_point(mut self, resume_point: NetworkPoint) -> Self {
        self.resume_point = Some(resume_point);
        self
    }

    /// Bound ingestion for diagnostics and tests.
    pub fn ingest_limits(mut self, until_slot: Option<Slot>, maximum_blocks: Option<usize>) -> Self {
        self.ingest_until_slot = until_slot;
        self.ingest_maximum_blocks = maximum_blocks;
        self
    }

    pub async fn synchronize(
        &self,
        cancellation: MithrilCancellation,
        observer: Arc<dyn MithrilObserver>,
    ) -> Result<MithrilSyncReport, MithrilSyncError> {
        if cancellation.is_cancelled() {
            return Err(MithrilSyncError::Cancelled);
        }
        observer.on_progress(MithrilProgress::StageChanged { stage: MithrilStage::ResolvingResumePoint });

        let target_dir = self.snapshots_dir.join(self.network.to_string());
        fs::create_dir_all(&target_dir).map_err(|source| store_error("create snapshot directory", source))?;
        validate_store_directories(&self.ledger_dir, &self.chain_dir)?;
        let _locks = acquire_sync_locks(&target_dir, &self.ledger_dir, &self.chain_dir)?;
        let chain_store: Arc<dyn ChainStore> = Arc::new(
            RocksDBStore::open(&RocksDbConfig::new(self.chain_dir.clone()))
                .map_err(|source| store_error("open chain store", source))?,
        );
        let resume_point = self.resolve_ledger_tip(chain_store.as_ref())?;

        let download_observer = observer.clone();
        let download = recover_then_download(
            chain_store.as_ref(),
            resume_point,
            self.resume_point,
            &cancellation,
            observer.as_ref(),
            || {
                download_from_mithril_for_range_with_observer(
                    self.network,
                    target_dir,
                    resume_point,
                    self.ingest_until_slot,
                    Arc::new(ForwardDownloadProgress(download_observer)),
                )
            },
        )
        .await?;
        if cancellation.is_cancelled() {
            return Err(MithrilSyncError::Cancelled);
        }

        let (final_point, processed_blocks) = self
            .ingest(chain_store.clone(), &download.immutable_dir, resume_point, &cancellation, observer.as_ref())
            .await?;

        drop(chain_store);
        if cancellation.is_cancelled() {
            return Err(MithrilSyncError::Cancelled);
        }
        let report =
            MithrilSyncReport { resume_point, final_point, snapshot_hash: download.snapshot_hash, processed_blocks };
        observer.on_progress(MithrilProgress::Completed { report: report.clone() });
        Ok(report)
    }

    fn resolve_ledger_tip(&self, chain_store: &dyn ChainStore) -> Result<Point, MithrilSyncError> {
        let ledger = ReadOnlyRocksDB::new(&RocksDbConfig::new(self.ledger_dir.clone()))
            .map_err(|source| store_error("open ledger store", source))?;
        let stored = NetworkPoint::from(ledger.tip().map_err(|source| store_error("read ledger tip", source))?);
        resolve_resume_point(chain_store, stored)
    }

    async fn ingest(
        &self,
        chain_store: Arc<dyn ChainStore>,
        immutable_dir: &Path,
        resume_point: Point,
        cancellation: &MithrilCancellation,
        observer: &dyn MithrilObserver,
    ) -> Result<(Point, u64), MithrilSyncError> {
        let ledger_config = ledger_config_for_network(self.network, self.ledger_dir.clone())?;
        let era_history = Arc::new(ledger_config.era_history.clone());
        let consensus_parameters = Arc::new(ledger_config.to_consensus_parameters());
        let state = make_state(&ledger_config, None, chain_store.clone())
            .map_err(|source| store_error("open ledger state", source))?;
        let stable_tip = state.tip().into_owned();
        if NetworkPoint::from(stable_tip) != NetworkPoint::from(resume_point) {
            return Err(MithrilSyncError::ResumePointMismatch {
                requested: NetworkPoint::from(resume_point),
                stored: NetworkPoint::from(stable_tip),
            });
        }
        let (pool_summaries_tx, pool_summaries_rx) = watch::channel(state.pool_summaries());
        let block_validator = make_block_validator(&ledger_config, state, chain_store.clone())
            .map_err(|source| store_error("start ledger worker", source))?;
        let ledger_stop = block_validator.thread_stop();
        block_validator.set_on_stake_dist_updated(Arc::new(move |summaries| {
            pool_summaries_tx.send_replace(summaries);
        }));

        observer.on_progress(MithrilProgress::StageChanged { stage: MithrilStage::Ingesting });
        let before = Instant::now();
        let result = self
            .ingest_blocks(
                immutable_dir,
                stable_tip,
                cancellation,
                observer,
                &chain_store,
                consensus_parameters,
                &block_validator,
                pool_summaries_rx,
                era_history,
            )
            .await;

        drop(block_validator);
        let shutdown = tokio::task::spawn_blocking(move || ledger_stop.join_timeout(LEDGER_SHUTDOWN_TIMEOUT))
            .await
            .map_err(|source| MithrilSyncError::WorkerShutdown { source: source.into() })?
            .map_err(|source| MithrilSyncError::WorkerShutdown { source: source.into() });
        shutdown?;

        let ledger_tip = self.resolve_ledger_tip(chain_store.as_ref())?;
        recover_stores(chain_store.as_ref(), ledger_tip)?;
        let (final_point, processed) = result?;
        let duration_seconds = Instant::now().saturating_duration_since(before).as_secs_f64();
        info!(
            cli::mithril::INGEST_COMPLETED,
            processed,
            duration_seconds,
            processed_per_seconds = processed as f64 / duration_seconds
        );
        Ok((final_point, processed))
    }

    #[expect(clippy::too_many_arguments)]
    async fn ingest_blocks(
        &self,
        immutable_dir: &Path,
        mut current_tip: Point,
        cancellation: &MithrilCancellation,
        observer: &dyn MithrilObserver,
        chain_store: &Arc<dyn ChainStore>,
        consensus_parameters: Arc<ConsensusParameters>,
        block_validator: &BlockValidator,
        mut pool_summaries: watch::Receiver<PoolSummaries>,
        era_history: Arc<EraHistory>,
    ) -> Result<(Point, u64), MithrilSyncError> {
        let blocks = read_blocks_after_point(immutable_dir, self.network, current_tip)
            .map_err(|source| MithrilSyncError::InvalidCache { source })?;
        let mut processed = 0_u64;
        for raw_block in blocks {
            if cancellation.is_cancelled() {
                return Err(MithrilSyncError::Cancelled);
            }
            let raw_block = RawBlock::from(
                raw_block.map_err(|source| MithrilSyncError::InvalidCache { source })?.into_boxed_slice(),
            );
            let network_block = NetworkBlock::try_from(raw_block.clone())
                .map_err(|source| MithrilSyncError::Validation { point: current_tip, source: source.into() })?;
            let block = network_block
                .decode_block()
                .map_err(|source| MithrilSyncError::Validation { point: current_tip, source: source.into() })?;
            let point = block.header.point();
            if self.ingest_until_slot.is_some_and(|until| point.slot_or_default() > until) {
                break;
            }
            process_block(
                chain_store,
                consensus_parameters.clone(),
                block_validator,
                &mut pool_summaries,
                era_history.clone(),
                cancellation,
                &raw_block,
                block,
            )
            .await?;
            current_tip = point;
            processed += 1;
            observer.on_progress(MithrilProgress::BlocksIngested { blocks: processed, point });
            if self.ingest_maximum_blocks.is_some_and(|maximum| processed as usize >= maximum) {
                break;
            }
        }
        if cancellation.is_cancelled() {
            return Err(MithrilSyncError::Cancelled);
        }
        Ok((current_tip, processed))
    }
}

fn resolve_resume_point(chain_store: &dyn ChainStore, stored: NetworkPoint) -> Result<Point, MithrilSyncError> {
    chain_store
        .load_point(&stored.hash())
        .filter(|point| NetworkPoint::from(point) == stored)
        .ok_or(MithrilSyncError::ResumePointNotFound { point: stored })
}

fn ledger_config_for_network(network: NetworkName, ledger_dir: PathBuf) -> Result<LedgerConfig, MithrilSyncError> {
    let era_history = network
        .as_era_history()
        .ok_or_else(|| store_error("resolve era history", anyhow!("unsupported network: {network}")))?;
    let global_parameters = network
        .as_global_parameters()
        .ok_or_else(|| store_error("resolve global parameters", anyhow!("unsupported network: {network}")))?;
    Ok(LedgerConfig {
        ledger_store: RocksDbConfig::new(ledger_dir),
        network,
        era_history: era_history.clone(),
        global_parameters: global_parameters.clone(),
        ..LedgerConfig::default()
    })
}

struct ForwardDownloadProgress(Arc<dyn MithrilObserver>);

impl MithrilDownloadObserver for ForwardDownloadProgress {
    fn on_progress(&self, progress: MithrilDownloadProgress) {
        self.0.on_progress(match progress {
            MithrilDownloadProgress::SnapshotSelected { hash, through_chunk } => {
                MithrilProgress::SnapshotSelected { hash, through_chunk }
            }
            MithrilDownloadProgress::Downloaded { downloaded_bytes, completed_files, total_files, total_bytes } => {
                MithrilProgress::Downloaded { downloaded_bytes, completed_files, total_files, total_bytes }
            }
            _ => return,
        });
    }
}

async fn recover_then_download<F, Download>(
    chain_store: &dyn ChainStore,
    resume_point: Point,
    requested: Option<NetworkPoint>,
    cancellation: &MithrilCancellation,
    observer: &dyn MithrilObserver,
    download: Download,
) -> Result<MithrilDownloadReport, MithrilSyncError>
where
    Download: FnOnce() -> F,
    F: Future<Output = Result<MithrilDownloadReport, MithrilDownloadError>>,
{
    if let Some(requested) = requested
        && requested != NetworkPoint::from(resume_point)
    {
        return Err(MithrilSyncError::ResumePointMismatch { requested, stored: NetworkPoint::from(resume_point) });
    }
    observer.on_progress(MithrilProgress::StageChanged { stage: MithrilStage::RecoveringStores });
    recover_stores(chain_store, resume_point)?;
    if cancellation.is_cancelled() {
        return Err(MithrilSyncError::Cancelled);
    }

    observer.on_progress(MithrilProgress::StageChanged { stage: MithrilStage::Downloading });
    tokio::select! {
        result = download() => result.map_err(classify_download_error),
        _ = cancellation.cancelled() => Err(MithrilSyncError::Cancelled),
    }
}

fn validate_store_directories(ledger_dir: &Path, chain_dir: &Path) -> Result<(), MithrilSyncError> {
    validate_store_directory(ledger_dir, "validate ledger store directory")?;
    validate_store_directory(chain_dir, "validate chain store directory")
}

fn validate_store_directory(path: &Path, operation: &'static str) -> Result<(), MithrilSyncError> {
    let metadata = fs::metadata(path).map_err(|source| store_error(operation, source))?;
    if metadata.is_dir() {
        Ok(())
    } else {
        Err(store_error(
            operation,
            io::Error::new(io::ErrorKind::NotADirectory, format!("{} is not a directory", path.display())),
        ))
    }
}

fn acquire_sync_locks(target_dir: &Path, ledger_dir: &Path, chain_dir: &Path) -> Result<Vec<File>, MithrilSyncError> {
    let mut directories = [target_dir, ledger_dir, chain_dir]
        .into_iter()
        .map(|directory| {
            fs::canonicalize(directory).map_err(|source| store_error("resolve synchronization lock path", source))
        })
        .collect::<Result<Vec<_>, _>>()?;
    directories.sort_unstable();
    directories.dedup();

    let mut legacy_locks = Vec::new();
    for directory in &directories {
        let legacy_path = directory.join(".mithril-sync.lock");
        let legacy_lock = match OpenOptions::new().read(true).write(true).open(&legacy_path) {
            Ok(lock) => lock,
            Err(source) if source.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => return Err(store_error("open legacy synchronization lock", source)),
        };
        lock_sync_file(&legacy_lock, directory)?;
        legacy_locks.push((legacy_path, legacy_lock));
    }

    let mut locks = Vec::with_capacity(directories.len());
    for directory in &directories {
        let lock = File::create(sync_lock_path(directory)?)
            .map_err(|source| store_error("create synchronization lock", source))?;
        lock_sync_file(&lock, directory)?;
        locks.push(lock);
    }

    for (legacy_path, legacy_lock) in legacy_locks {
        drop(legacy_lock);
        match fs::remove_file(legacy_path) {
            Ok(()) => {}
            Err(source) if source.kind() == io::ErrorKind::NotFound => {}
            Err(source) => return Err(store_error("remove legacy synchronization lock", source)),
        }
    }

    Ok(locks)
}

fn sync_lock_path(directory: &Path) -> Result<PathBuf, MithrilSyncError> {
    let name = directory.file_name().ok_or_else(|| {
        store_error(
            "resolve synchronization lock path",
            io::Error::new(io::ErrorKind::InvalidInput, format!("cannot lock store root {}", directory.display())),
        )
    })?;
    let mut lock_name = OsString::from(".");
    lock_name.push(name);
    lock_name.push(".mithril-sync.lock");
    Ok(directory.with_file_name(lock_name))
}

fn lock_sync_file(lock: &File, directory: &Path) -> Result<(), MithrilSyncError> {
    match lock.try_lock() {
        Ok(()) => Ok(()),
        Err(TryLockError::WouldBlock) => Err(MithrilSyncError::Concurrent { path: directory.to_path_buf() }),
        Err(source) => Err(store_error("acquire synchronization lock", source)),
    }
}

fn recover_stores(chain_store: &dyn ChainStore, ledger_tip: Point) -> Result<(), MithrilSyncError> {
    let chain_tip = chain_store.get_best_chain_tip();
    if ledger_tip != chain_tip {
        let recovery = if can_adopt(chain_store, ledger_tip, chain_tip) {
            info!(cli::mithril::RECOVER_CHAIN_TIP, ledger_tip, chain_tip);
            adopt_validated_block(chain_store, ledger_tip)
        } else {
            realign_chain_store_to(chain_store, ledger_tip, ClearValidity::ValidOnly)
        };
        recovery.map_err(|source| {
            MithrilSyncError::RebootstrapRequired(Box::new(RebootstrapRequired {
                ledger_tip,
                chain_tip,
                reason: source.to_string(),
            }))
        })?;
    }
    Ok(())
}

fn can_adopt(chain_store: &dyn ChainStore, ledger_tip: Point, chain_tip: Point) -> bool {
    chain_store.load_header_with_validity(&ledger_tip.hash()).is_some_and(|(header, validity)| {
        header.point() == ledger_tip
            && validity != Some(false)
            && header.parent_hash().unwrap_or(ORIGIN_HASH) == chain_tip.hash()
            && chain_store.get_nonces(&ledger_tip.hash()).is_some()
    })
}

fn adopt_validated_block(chain_store: &dyn ChainStore, point: Point) -> anyhow::Result<()> {
    chain_store.set_block_valid(&point.hash(), true)?;
    chain_store.roll_forward_chain(&point)?;
    let chain_tip = chain_store.get_best_chain_tip();
    if chain_tip != point {
        anyhow::bail!("adopted chain tip {chain_tip} does not match ledger tip {point}");
    }
    Ok(())
}

#[expect(clippy::too_many_arguments)]
async fn process_block(
    chain_store: &Arc<dyn ChainStore>,
    consensus_parameters: Arc<ConsensusParameters>,
    block_validator: &BlockValidator,
    pool_summaries: &mut watch::Receiver<PoolSummaries>,
    era_history: Arc<EraHistory>,
    cancellation: &MithrilCancellation,
    raw_block: &RawBlock,
    block: Block,
) -> Result<(), MithrilSyncError> {
    let point = block.header.point();
    // Keep adoption last: an interruption after the ledger commit then leaves its tip off the
    // adopted chain, which normal startup detects before opening the node.
    chain_store
        .store_block(&point.hash(), raw_block)
        .map_err(|source| MithrilSyncError::Validation { point, source: source.into() })?;
    let nonces = loop {
        let summaries = Arc::new(pool_summaries.borrow_and_update().clone());
        match validate_header(
            &block.header,
            consensus_parameters.clone(),
            chain_store.clone(),
            summaries,
            era_history.clone(),
        ) {
            Ok(nonces) => break nonces,
            Err(source) => {
                let Some(target) = source.missing_stake_distribution() else {
                    return Err(MithrilSyncError::Validation { point, source: source.into() });
                };
                if !wait_for_stake_distribution(pool_summaries, target, cancellation).await? {
                    return Err(MithrilSyncError::Validation { point, source: source.into() });
                }
            }
        }
    };
    chain_store
        .store_validated_header(&block.header, &nonces)
        .map_err(|source| MithrilSyncError::Validation { point, source: source.into() })?;
    block_validator
        .roll_forward_block(block)
        .await
        .map_err(|source| MithrilSyncError::Validation {
            point,
            source: anyhow!("ledger worker failed at {point}: {source:?}"),
        })?
        .map_err(|source| MithrilSyncError::Validation {
            point,
            source: anyhow!("ledger rejected block at {point}: {source:?}"),
        })?;
    adopt_validated_block(chain_store.as_ref(), point)
        .map_err(|source| MithrilSyncError::Validation { point, source })?;
    Ok(())
}

async fn wait_for_stake_distribution(
    pool_summaries: &mut watch::Receiver<PoolSummaries>,
    target: amaru_kernel::Epoch,
    cancellation: &MithrilCancellation,
) -> Result<bool, MithrilSyncError> {
    loop {
        if pool_summaries.borrow().by_epoch.contains_key(&target) {
            return Ok(true);
        }
        tokio::select! {
            changed = pool_summaries.changed() => {
                if changed.is_err() {
                    return Ok(false);
                }
            }
            _ = cancellation.cancelled() => return Err(MithrilSyncError::Cancelled),
        }
    }
}

fn store_error(operation: &'static str, source: impl Into<anyhow::Error>) -> MithrilSyncError {
    MithrilSyncError::Store { operation, source: source.into() }
}

fn classify_download_error(source: MithrilDownloadError) -> MithrilSyncError {
    match source {
        MithrilDownloadError::Unavailable { source } => MithrilSyncError::SnapshotUnavailable { source },
        MithrilDownloadError::Inapplicable { through_chunk, required_chunk } => {
            MithrilSyncError::SnapshotInapplicable { through_chunk, required_chunk }
        }
        MithrilDownloadError::InvalidCache { source } => MithrilSyncError::InvalidCache { source },
        MithrilDownloadError::Validation { source } => MithrilSyncError::SnapshotValidation { source },
        MithrilDownloadError::Download(source) => MithrilSyncError::Download { source },
        source => MithrilSyncError::Download { source: source.into() },
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Mutex};

    use amaru_kernel::{Epoch, Header, make_header};
    use amaru_ouroboros::{BaseReadChainStore, Nonces, WriteChainStore, in_memory_chain_store::InMemoryChainStore};
    use tempfile::tempdir;

    use super::*;

    #[derive(Default)]
    struct RecordingObserver(Mutex<Vec<MithrilProgress>>);

    impl MithrilObserver for RecordingObserver {
        fn on_progress(&self, progress: MithrilProgress) {
            self.0.lock().unwrap().push(progress);
        }
    }

    #[tokio::test]
    async fn cancelled_synchronization_never_reports_completion() {
        let directory = tempdir().unwrap();
        let cancellation = MithrilCancellation::new();
        cancellation.cancel();
        let observer = Arc::new(RecordingObserver::default());
        let synchronizer = MithrilSynchronizer::new(
            NetworkName::Preprod,
            directory.path().join("ledger"),
            directory.path().join("chain"),
            directory.path().join("snapshots"),
        );

        assert!(matches!(
            synchronizer.synchronize(cancellation, observer.clone()).await,
            Err(MithrilSyncError::Cancelled)
        ));
        assert!(!observer.0.lock().unwrap().iter().any(|event| matches!(event, MithrilProgress::Completed { .. })));
    }

    #[tokio::test]
    async fn recovery_precedes_an_unavailable_download() {
        let from = make_header(1, 1, None);
        let target = make_header(2, 2, Some(from.hash()));
        let store = InMemoryChainStore::new();
        store.store_header(&from).unwrap();
        store.roll_forward_chain(&from.point()).unwrap();
        store.store_validated_header(&target, &Nonces::for_tests()).unwrap();
        let observer = RecordingObserver::default();
        observer.on_progress(MithrilProgress::StageChanged { stage: MithrilStage::ResolvingResumePoint });

        let result = recover_then_download(
            &store,
            target.point(),
            Some(NetworkPoint::from(target.point())),
            &MithrilCancellation::new(),
            &observer,
            || async {
                assert_eq!(store.get_best_chain_tip(), target.point());
                Err(MithrilDownloadError::Unavailable { source: anyhow!("offline") })
            },
        )
        .await;

        assert!(matches!(result, Err(MithrilSyncError::SnapshotUnavailable { .. })));
        assert_eq!(store.get_best_chain_tip(), target.point());
        assert_eq!(
            observer
                .0
                .lock()
                .unwrap()
                .iter()
                .filter_map(|progress| match progress {
                    MithrilProgress::StageChanged { stage } => Some(*stage),
                    MithrilProgress::SnapshotSelected { .. }
                    | MithrilProgress::Downloaded { .. }
                    | MithrilProgress::BlocksIngested { .. }
                    | MithrilProgress::Completed { .. } => None,
                })
                .collect::<Vec<_>>(),
            [MithrilStage::ResolvingResumePoint, MithrilStage::RecoveringStores, MithrilStage::Downloading]
        );
    }

    #[test]
    fn synchronization_lock_covers_the_store_pair_across_cache_directories() {
        let directory = tempdir().unwrap();
        let ledger_dir = directory.path().join("ledger");
        let chain_dir = directory.path().join("chain");
        let first_cache = directory.path().join("cache-a");
        let second_cache = directory.path().join("cache-b");
        for path in [&ledger_dir, &chain_dir, &first_cache, &second_cache] {
            fs::create_dir(path).unwrap();
        }
        File::create(ledger_dir.join(".mithril-sync.lock")).unwrap();

        let locks = acquire_sync_locks(&first_cache, &ledger_dir, &chain_dir).unwrap();
        assert!(!ledger_dir.join(".mithril-sync.lock").exists());
        assert!(directory.path().join(".ledger.mithril-sync.lock").exists());
        assert!(directory.path().join(".chain.mithril-sync.lock").exists());
        assert!(matches!(
            acquire_sync_locks(&second_cache, &ledger_dir, &chain_dir),
            Err(MithrilSyncError::Concurrent { .. })
        ));
        drop(locks);
        acquire_sync_locks(&second_cache, &ledger_dir, &chain_dir).unwrap();
    }

    #[test]
    fn synchronization_respects_an_active_legacy_lock_before_removing_it() {
        let directory = tempdir().unwrap();
        let ledger_dir = directory.path().join("ledger");
        let chain_dir = directory.path().join("chain");
        let cache_dir = directory.path().join("cache");
        for path in [&ledger_dir, &chain_dir, &cache_dir] {
            fs::create_dir(path).unwrap();
        }
        let legacy_path = ledger_dir.join(".mithril-sync.lock");
        let legacy_lock = File::create(&legacy_path).unwrap();
        legacy_lock.try_lock().unwrap();

        assert!(matches!(
            acquire_sync_locks(&cache_dir, &ledger_dir, &chain_dir),
            Err(MithrilSyncError::Concurrent { .. })
        ));
        assert!(legacy_path.exists());

        drop(legacy_lock);
        acquire_sync_locks(&cache_dir, &ledger_dir, &chain_dir).unwrap();
        assert!(!legacy_path.exists());
    }

    #[derive(Clone, Copy)]
    enum InterruptedAfter {
        BeforeWrites,
        HeaderAndNonces,
        LedgerCommit,
        Validity,
        ChainAdoption,
    }

    #[test]
    fn recovery_handles_each_cross_store_write_boundary() {
        for boundary in [
            InterruptedAfter::BeforeWrites,
            InterruptedAfter::HeaderAndNonces,
            InterruptedAfter::LedgerCommit,
            InterruptedAfter::Validity,
            InterruptedAfter::ChainAdoption,
        ] {
            assert_boundary_is_recoverable(boundary);
        }
    }

    fn assert_boundary_is_recoverable(boundary: InterruptedAfter) {
        let from = make_header(1, 1, None);
        let target = make_header(2, 2, Some(from.hash()));
        let store = InMemoryChainStore::new();
        store.store_header(&from).unwrap();
        store.roll_forward_chain(&from.point()).unwrap();

        if !matches!(boundary, InterruptedAfter::BeforeWrites) {
            store.store_validated_header(&target, &Nonces::for_tests()).unwrap();
        }
        let ledger_tip = match boundary {
            InterruptedAfter::BeforeWrites | InterruptedAfter::HeaderAndNonces => from.point(),
            InterruptedAfter::LedgerCommit | InterruptedAfter::Validity | InterruptedAfter::ChainAdoption => {
                target.point()
            }
        };
        if matches!(boundary, InterruptedAfter::Validity | InterruptedAfter::ChainAdoption) {
            store.set_block_valid(&target.hash(), true).unwrap();
        }
        if matches!(boundary, InterruptedAfter::ChainAdoption) {
            store.roll_forward_chain(&target.point()).unwrap();
        }

        recover_stores(&store, ledger_tip).unwrap();

        assert_eq!(store.get_best_chain_tip(), ledger_tip);
    }

    #[test]
    fn recovery_requires_rebootstrap_when_the_ledger_target_is_missing() {
        let from: Header = make_header(1, 1, None);
        let target = make_header(2, 2, Some(from.hash()));
        let store = InMemoryChainStore::new();
        store.store_header(&from).unwrap();
        store.roll_forward_chain(&from.point()).unwrap();

        assert!(matches!(recover_stores(&store, target.point()), Err(MithrilSyncError::RebootstrapRequired(_))));
    }

    #[test]
    fn resolves_the_bootstrap_ledger_tip_height_from_the_chain_store() {
        let tip = make_header(42, 123, None);
        let store = InMemoryChainStore::new();
        store.store_header(&tip).unwrap();

        let stored_tip = NetworkPoint::from(tip.point());
        let resolved = resolve_resume_point(&store, stored_tip).unwrap();

        assert_eq!(resolved, tip.point());
        assert_eq!(resolved.block_height(), 42.into());
    }

    #[test]
    fn recovery_initializes_a_bootstrapped_store_without_a_best_chain() {
        let parent = make_header(41, 122, None);
        let tip = make_header(42, 123, Some(parent.hash()));
        let store = InMemoryChainStore::new();
        store.store_validated_header(&parent, &Nonces::for_tests()).unwrap();
        store.store_validated_header(&tip, &Nonces::for_tests()).unwrap();
        assert_eq!(store.get_best_chain_tip(), Point::Origin);

        recover_stores(&store, tip.point()).unwrap();

        assert_eq!(store.get_anchor_point(), tip.point());
        assert_eq!(store.get_best_chain_tip(), tip.point());
        assert!(store.is_on_best_chain(NetworkPoint::from(tip.point())));
    }

    #[test]
    fn recovery_rewinds_the_adopted_chain_to_the_durable_ledger_tip() {
        let durable = make_header(1, 1, None);
        let volatile_1 = make_header(2, 2, Some(durable.hash()));
        let volatile_2 = make_header(3, 3, Some(volatile_1.hash()));
        let store = InMemoryChainStore::new();
        for header in [&durable, &volatile_1, &volatile_2] {
            store.store_validated_header(header, &Nonces::for_tests()).unwrap();
            store.set_block_valid(&header.hash(), true).unwrap();
            store.roll_forward_chain(&header.point()).unwrap();
        }

        recover_stores(&store, durable.point()).unwrap();

        assert_eq!(store.get_anchor_point(), durable.point());
        assert_eq!(store.get_best_chain_tip(), durable.point());
        assert_eq!(store.load_header_with_validity(&volatile_1.hash()).unwrap().1, None);
        assert_eq!(store.load_header_with_validity(&volatile_2.hash()).unwrap().1, None);
    }

    #[tokio::test]
    async fn replay_waits_for_a_background_stake_distribution() {
        let target = Epoch::from(1120);
        let (sender, mut receiver) = watch::channel(PoolSummaries::default());
        let update = tokio::spawn(async move {
            tokio::task::yield_now().await;
            sender.send_modify(|summaries| {
                summaries.by_epoch.insert(target, BTreeMap::new());
            });
        });

        assert!(wait_for_stake_distribution(&mut receiver, target, &MithrilCancellation::new()).await.unwrap());
        update.await.unwrap();
    }
}
