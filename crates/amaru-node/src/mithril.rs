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
    fs::{self, File, TryLockError},
    future::Future,
    io::{self, IsTerminal},
    panic::AssertUnwindSafe,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use amaru_consensus::{
    block_validator::{BlockValidator, LedgerThreadStop},
    validate_header::validate_header,
};
use amaru_kernel::{
    Block, ConsensusParameters, EraHistory, IsHeader, NetworkName, NetworkPoint, ORIGIN_HASH, Point, RawBlock, Slot,
    cardano::network_block::NetworkBlock,
};
use amaru_ledger::store::ReadStore;
use amaru_mithril::{
    MithrilDownloadError, MithrilDownloadObserver, MithrilDownloadProgress, MithrilDownloadReport,
    MithrilDownloadStage, download_from_mithril_for_range_with_observer, read_blocks_after_point,
};
use amaru_observability::info;
use amaru_ouroboros::{ChainStore, PoolSummaries, can_validate_blocks::CanValidateBlocks};
use amaru_progress_bar::{ProgressBar, TerminalProgressBar};
use amaru_stores::rocksdb::{ReadOnlyRocksDB, RocksDbConfig};
use anyhow::anyhow;
use futures_util::FutureExt;
use thiserror::Error;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::{
    ClearValidity, NodeStartError,
    chain_realign::ensure_store_consistency,
    realign_chain_store_to,
    stages::{
        build_node::{ledger_store_error, make_block_validator, make_state, open_chain_store},
        config::LedgerConfig,
    },
};

const LEDGER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
const STRUCTURED_PROGRESS_INTERVAL: Duration = Duration::from_secs(5);

/// Cooperative cancellation for [`MithrilSynchronizer::synchronize`].
///
/// Cancel the token, then await synchronization completion and inspect its result.
/// [`MithrilSyncError::WorkerShutdown`] requires a process restart before using the stores again.
pub type MithrilCancellation = CancellationToken;

/// A high-level stage of Mithril synchronization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MithrilStage {
    ResolvingResumePoint,
    FetchingSnapshot,
    ValidatingCertificate,
    Downloading,
    VerifyingDatabase { from_chunk: u64, through_chunk: u64, files: u64 },
    DatabaseVerified,
    RecoveringStores,
    Ingesting,
}

impl MithrilStage {
    fn as_str(self) -> &'static str {
        match self {
            Self::ResolvingResumePoint => "resolving_resume_point",
            Self::FetchingSnapshot => "fetching_snapshot",
            Self::ValidatingCertificate => "validating_certificate",
            Self::Downloading => "downloading",
            Self::VerifyingDatabase { .. } => "verifying_database",
            Self::DatabaseVerified => "database_verified",
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
    StageChanged {
        stage: MithrilStage,
    },
    SnapshotSelected {
        hash: String,
        through_chunk: u64,
    },
    /// One certificate in the snapshot certificate chain has been validated.
    CertificateValidated,
    Downloaded {
        downloaded_bytes: u64,
        completed_files: u64,
        total_files: u64,
        total_bytes: Option<u64>,
    },
    BlocksIngested {
        blocks: u64,
        point: Point,
    },
    Completed {
        report: MithrilSyncReport,
    },
}

/// Passive consumer of canonical synchronization progress.
///
/// Callbacks run on the synchronization task and should return promptly. A panic during ingestion
/// stops replay and runs worker shutdown and store reconciliation before reporting a task failure.
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
        match &mut *self.renderer.lock().unwrap_or_else(std::sync::PoisonError::into_inner) {
            DefaultRenderer::Terminal(renderer) => renderer.render(progress),
            DefaultRenderer::Structured(renderer) => renderer.render(progress),
        }
    }
}

enum DefaultRenderer {
    Terminal(TerminalRenderer),
    Structured(StructuredRenderer),
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
        let (last_rendered, completed) = match progress {
            MithrilProgress::Downloaded { completed_files, .. } => {
                let completed = *completed_files > self.completed_files;
                self.completed_files = *completed_files;
                (&mut self.last_download_at, completed)
            }
            MithrilProgress::BlocksIngested { .. } => (&mut self.last_ingest_at, false),
            MithrilProgress::StageChanged { .. }
            | MithrilProgress::SnapshotSelected { .. }
            | MithrilProgress::CertificateValidated
            | MithrilProgress::Completed { .. } => return true,
        };
        let should_render =
            completed || last_rendered.is_none_or(|last| now.duration_since(last) >= STRUCTURED_PROGRESS_INTERVAL);
        if should_render {
            *last_rendered = Some(now);
        }
        should_render
    }
}

#[derive(Default)]
struct TerminalRenderer {
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    download: Option<Box<dyn ProgressBar>>,
    verification: Option<Box<dyn ProgressBar>>,
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
            MithrilProgress::StageChanged { stage } => {
                if let Some(verification) = self.verification.take() {
                    verification.clear();
                }
                if stage != MithrilStage::Downloading
                    && let Some(download) = self.download.take()
                {
                    download.finish();
                }
                let template = match stage {
                    MithrilStage::ValidatingCertificate => Some(
                        "{spinner:.green} {elapsed_precise} validating Mithril certificate chain ({pos} certificates)",
                    ),
                    MithrilStage::VerifyingDatabase { .. } => {
                        Some("{spinner:.green} {elapsed_precise} verifying Mithril database")
                    }
                    MithrilStage::ResolvingResumePoint
                    | MithrilStage::FetchingSnapshot
                    | MithrilStage::Downloading
                    | MithrilStage::DatabaseVerified
                    | MithrilStage::RecoveringStores
                    | MithrilStage::Ingesting => None,
                };
                self.verification = template.map(|template| TerminalProgressBar::new(0_u64, template).boxed());
            }
            MithrilProgress::CertificateValidated => {
                if let Some(verification) = &self.verification {
                    verification.increment();
                }
            }
            MithrilProgress::Completed { .. } => {
                if let Some(download) = self.download.take() {
                    download.finish();
                }
            }
            MithrilProgress::SnapshotSelected { .. } | MithrilProgress::BlocksIngested { .. } => {}
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
        MithrilProgress::CertificateValidated => {}
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
    #[error("cannot open synchronization stores: {0}")]
    Startup(#[source] NodeStartError),
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
    /// An operational failure, not evidence that rebuilding the stores is necessary.
    #[error("store operation failed: {operation}")]
    Store {
        operation: &'static str,
        #[source]
        source: anyhow::Error,
    },
    /// The store state is outside the supported recovery cases.
    #[error("interrupted store mutation cannot be recovered: {0}")]
    RebootstrapRequired(Box<RebootstrapRequired>),
    #[error("Mithril synchronization task failed")]
    TaskFailed {
        #[source]
        source: tokio::task::JoinError,
    },
    /// Worker termination and store release were not confirmed.
    ///
    /// Preserve this error and require a process restart. Do not reopen, recover, rebuild, or
    /// delete these stores in this process: the worker may still own or write them. This error
    /// takes precedence over cancellation, ingestion failure, and a rebootstrap recommendation.
    #[error("ledger worker termination and store release were not confirmed; process restart required")]
    WorkerShutdown {
        #[source]
        source: anyhow::Error,
    },
}

/// Outcome of reconciling the adopted chain with the durable ledger tip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StoreRecoveryOutcome {
    AlreadyConsistent {
        point: Point,
    },
    /// The adopted chain moved from `before` to `after`; the durable ledger was unchanged.
    Recovered {
        before: Point,
        after: Point,
    },
}

impl StoreRecoveryOutcome {
    /// The common ledger and adopted-chain tip after recovery.
    pub fn point(self) -> Point {
        match self {
            Self::AlreadyConsistent { point } | Self::Recovered { after: point, .. } => point,
        }
    }
}

/// Recover an interrupted store pair without network access or a snapshot cache.
///
/// Call this while the node is stopped, then await completion before opening its stores. All store
/// handles and recovery locks are released before returning. Synchronization uses the same recovery
/// implementation. [`MithrilSyncError::RebootstrapRequired`] identifies unsupported store states;
/// operational failures return [`MithrilSyncError::Store`] or [`MithrilSyncError::Startup`].
///
/// Store operations run on a Tokio blocking task. Once started, that task retains its locks and
/// finishes recovery even if the caller drops this future. Do not call this after
/// [`MithrilSyncError::WorkerShutdown`] until the process has restarted.
///
/// ```no_run
/// # async fn example() -> Result<(), amaru_node::MithrilSyncError> {
/// let outcome = amaru_node::recover_store_pair("ledger.preprod.db", "chain.preprod.db").await?;
/// # let _ = outcome;
/// # Ok(())
/// # }
/// ```
pub async fn recover_store_pair(
    ledger_dir: impl Into<PathBuf>,
    chain_dir: impl Into<PathBuf>,
) -> Result<StoreRecoveryOutcome, MithrilSyncError> {
    let ledger_dir = ledger_dir.into();
    let chain_dir = chain_dir.into();
    tokio::task::spawn_blocking(move || {
        validate_store_directories(&ledger_dir, &chain_dir)?;
        let _locks = acquire_sync_locks([ledger_dir.as_path(), chain_dir.as_path()])?;
        let chain_store = open_chain_store(&RocksDbConfig::new(chain_dir), false).map_err(MithrilSyncError::Startup)?;
        let ledger_tip = resolve_ledger_tip(&ledger_dir, &chain_store)?;
        recover_stores(&chain_store, ledger_tip)
    })
    .await
    .map_err(|source| MithrilSyncError::TaskFailed { source })?
}

/// Recover stores and return their common tip; see [`recover_store_pair`] for the detailed outcome
/// and the requirement to restart after [`MithrilSyncError::WorkerShutdown`].
pub async fn reconcile_mithril_stores(
    ledger_dir: impl Into<PathBuf>,
    chain_dir: impl Into<PathBuf>,
) -> Result<Point, MithrilSyncError> {
    recover_store_pair(ledger_dir, chain_dir).await.map(StoreRecoveryOutcome::point)
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
#[derive(Clone)]
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
    ///
    /// A zero block limit skips ingestion and leaves the resume point unchanged.
    pub fn ingest_limits(mut self, until_slot: Option<Slot>, maximum_blocks: Option<usize>) -> Self {
        self.ingest_until_slot = until_slot;
        self.ingest_maximum_blocks = maximum_blocks;
        self
    }

    /// Synchronize stores with a verified Mithril snapshot.
    ///
    /// To cancel, cancel the token and then await completion. Completion includes ledger worker
    /// shutdown and reconciliation of the chain store with the durable ledger tip. Inspect the
    /// result: [`MithrilSyncError::WorkerShutdown`] means termination and store release were not
    /// confirmed. Preserve that error and require a process restart; do not reopen, recover,
    /// rebuild, or delete the stores in this process. It takes precedence over ingestion errors.
    ///
    /// Dropping or aborting this future requests cancellation of an owned synchronization task.
    /// That task retains the stores and synchronization locks until cleanup finishes. Keep the
    /// Tokio runtime alive for cleanup; runtime shutdown or process termination can interrupt it.
    pub async fn synchronize(
        &self,
        cancellation: MithrilCancellation,
        observer: Arc<dyn MithrilObserver>,
    ) -> Result<MithrilSyncReport, MithrilSyncError> {
        let synchronizer = self.clone();
        run_synchronization(cancellation, move |cancellation| async move {
            synchronizer.synchronize_inner(cancellation, observer).await
        })
        .await
    }

    async fn synchronize_inner(
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
        let _locks = acquire_sync_locks([&target_dir, &self.ledger_dir, &self.chain_dir])?;
        let chain_store: Arc<dyn ChainStore> = Arc::new(
            open_chain_store(&RocksDbConfig::new(self.chain_dir.clone()), false).map_err(MithrilSyncError::Startup)?,
        );
        let resume_point = resolve_ledger_tip(&self.ledger_dir, chain_store.as_ref())?;

        let download = recover_then_download(
            chain_store.as_ref(),
            resume_point,
            self.resume_point,
            &cancellation,
            observer.as_ref(),
            download_from_mithril_for_range_with_observer(
                self.network,
                target_dir,
                resume_point,
                self.ingest_until_slot,
                Arc::new(ForwardDownloadProgress(observer.clone())),
            ),
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

    async fn ingest(
        &self,
        chain_store: Arc<dyn ChainStore>,
        immutable_dir: &Path,
        resume_point: Point,
        cancellation: &MithrilCancellation,
        observer: &dyn MithrilObserver,
    ) -> Result<(Point, u64), MithrilSyncError> {
        if self.ingest_maximum_blocks == Some(0) {
            return Ok((resume_point, 0));
        }
        let ledger_config = ledger_config_for_network(self.network, self.ledger_dir.clone())?;
        let era_history = Arc::new(ledger_config.era_history.clone());
        let consensus_parameters = Arc::new(ledger_config.to_consensus_parameters());
        let state = make_state(&ledger_config, None, chain_store.clone())
            .map_err(|source| MithrilSyncError::Startup(source.into()))?;
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

        let before = Instant::now();
        let ingestion_store = chain_store.clone();
        let ingestion = async move {
            let chain_store = ingestion_store;
            observer.on_progress(MithrilProgress::StageChanged { stage: MithrilStage::Ingesting });
            let mut current_tip = stable_tip;
            let mut pool_summaries = pool_summaries_rx;
            let blocks = read_blocks_after_point(immutable_dir, self.network, current_tip)
                .map_err(|source| MithrilSyncError::InvalidCache { source })?;
            let mut processed = 0_u64;
            for raw_block in blocks.take(self.ingest_maximum_blocks.unwrap_or(usize::MAX)) {
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
                    &chain_store,
                    consensus_parameters.clone(),
                    &block_validator,
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
            }
            if cancellation.is_cancelled() {
                return Err(MithrilSyncError::Cancelled);
            }
            Ok((current_tip, processed))
        };
        let cleanup = async {
            stop_ledger_worker(ledger_stop, LEDGER_SHUTDOWN_TIMEOUT).await?;

            let ledger_tip = resolve_ledger_tip(&self.ledger_dir, chain_store.as_ref())?;
            recover_stores(chain_store.as_ref(), ledger_tip).map(|_| ())
        };
        let (final_point, processed) = complete_ingestion(ingestion, cleanup).await?;
        let duration_seconds = Instant::now().saturating_duration_since(before).as_secs_f64();
        info!(
            cli::mithril::INGEST_COMPLETED,
            processed,
            duration_seconds,
            processed_per_seconds = processed as f64 / duration_seconds
        );
        Ok((final_point, processed))
    }
}

/// The caller owns cancellation; the spawned task owns stores, locks, and cleanup.
async fn run_synchronization<T, F>(
    cancellation: MithrilCancellation,
    synchronize: impl FnOnce(MithrilCancellation) -> F,
) -> Result<T, MithrilSyncError>
where
    T: Send + 'static,
    F: Future<Output = Result<T, MithrilSyncError>> + Send + 'static,
{
    let cancellation = cancellation.child_token();
    let cancel_on_drop = cancellation.clone().drop_guard();
    let result = tokio::spawn(synchronize(cancellation)).await;
    cancel_on_drop.disarm();
    result.map_err(|source| MithrilSyncError::TaskFailed { source })?
}

async fn stop_ledger_worker(ledger_stop: LedgerThreadStop, timeout: Duration) -> Result<(), MithrilSyncError> {
    tokio::task::spawn_blocking(move || ledger_stop.join_timeout(timeout))
        .await
        .map_err(|source| MithrilSyncError::WorkerShutdown { source: source.into() })?
        .map_err(|source| MithrilSyncError::WorkerShutdown { source: source.into() })
}

/// Drop the ingestion future (and its validator) before joining the worker, even on panic.
async fn complete_ingestion<T>(
    ingestion: impl Future<Output = Result<T, MithrilSyncError>>,
    cleanup: impl Future<Output = Result<(), MithrilSyncError>>,
) -> Result<T, MithrilSyncError> {
    let result = AssertUnwindSafe(ingestion).catch_unwind().await;
    cleanup.await?;
    match result {
        Ok(result) => result,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

fn resolve_ledger_tip(ledger_dir: &Path, chain_store: &dyn ChainStore) -> Result<Point, MithrilSyncError> {
    let ledger = ReadOnlyRocksDB::new(&RocksDbConfig::new(ledger_dir.to_path_buf()))
        .map_err(|source| MithrilSyncError::Startup(ledger_store_error(source)))?;
    let stored = NetworkPoint::from(ledger.tip().map_err(|source| store_error("read ledger tip", source))?);
    resolve_resume_point(chain_store, stored)
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

#[allow(clippy::wildcard_enum_match_arm)]
impl MithrilDownloadObserver for ForwardDownloadProgress {
    fn on_progress(&self, progress: MithrilDownloadProgress) {
        self.0.on_progress(match progress {
            MithrilDownloadProgress::StageChanged { stage } => MithrilProgress::StageChanged {
                stage: match stage {
                    MithrilDownloadStage::FetchingSnapshot => MithrilStage::FetchingSnapshot,
                    MithrilDownloadStage::ValidatingCertificate => MithrilStage::ValidatingCertificate,
                    MithrilDownloadStage::Downloading { .. } => MithrilStage::Downloading,
                    MithrilDownloadStage::VerifyingDatabase { from_chunk, through_chunk, files } => {
                        MithrilStage::VerifyingDatabase { from_chunk, through_chunk, files }
                    }
                    MithrilDownloadStage::DatabaseVerified => MithrilStage::DatabaseVerified,
                    _ => return,
                },
            },
            MithrilDownloadProgress::CertificateValidated => MithrilProgress::CertificateValidated,
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

async fn recover_then_download(
    chain_store: &dyn ChainStore,
    resume_point: Point,
    requested: Option<NetworkPoint>,
    cancellation: &MithrilCancellation,
    observer: &dyn MithrilObserver,
    download: impl Future<Output = Result<MithrilDownloadReport, MithrilDownloadError>>,
) -> Result<MithrilDownloadReport, MithrilSyncError> {
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
        result = download => result.map_err(classify_download_error),
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

fn acquire_sync_locks<const N: usize>(directories: [&Path; N]) -> Result<Vec<File>, MithrilSyncError> {
    let mut directories = directories
        .into_iter()
        .map(|directory| {
            fs::canonicalize(directory).map_err(|source| store_error("resolve synchronization lock path", source))
        })
        .collect::<Result<Vec<_>, _>>()?;
    directories.sort_unstable();
    directories.dedup();

    directories
        .iter()
        .map(|directory| {
            let lock = File::create(sync_lock_path(directory)?)
                .map_err(|source| store_error("create synchronization lock", source))?;
            lock_sync_file(&lock, directory)?;
            Ok(lock)
        })
        .collect()
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

fn recover_stores(chain_store: &dyn ChainStore, ledger_tip: Point) -> Result<StoreRecoveryOutcome, MithrilSyncError> {
    let chain_tip = chain_store.get_best_chain_tip();
    if ledger_tip == chain_tip {
        return Ok(StoreRecoveryOutcome::AlreadyConsistent { point: ledger_tip });
    }
    if can_adopt(chain_store, ledger_tip, chain_tip) {
        info!(cli::mithril::RECOVER_CHAIN_TIP, ledger_tip, chain_tip);
        adopt_validated_block(chain_store, ledger_tip)
            .map_err(|source| store_error("adopt recovered ledger tip", source))?;
    } else {
        ensure_store_consistency(chain_store, ledger_tip).map_err(|source| {
            MithrilSyncError::RebootstrapRequired(Box::new(RebootstrapRequired {
                ledger_tip,
                chain_tip,
                reason: source.to_string(),
            }))
        })?;
        realign_chain_store_to(chain_store, ledger_tip, ClearValidity::ValidOnly)
            .map_err(|source| store_error("realign chain store to ledger tip", source))?;
    }
    Ok(StoreRecoveryOutcome::Recovered { before: chain_tip, after: ledger_tip })
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
    use amaru_ouroboros::{
        BaseReadChainStore, Nonces, StoreError, WriteChainStore, in_memory_chain_store::InMemoryChainStore,
        overriding_chain_store::OverridingChainStore,
    };
    use amaru_stores::rocksdb::{RocksDB, consensus::RocksDBStore};
    use tempfile::tempdir;
    use test_case::test_case;

    use super::*;
    use crate::tests::configuration::NodeTestConfig;

    #[derive(Default)]
    struct RecordingObserver(Mutex<Vec<MithrilProgress>>);

    impl MithrilObserver for RecordingObserver {
        fn on_progress(&self, progress: MithrilProgress) {
            self.0.lock().unwrap().push(progress);
        }
    }

    fn interrupted_ingestion_store() -> (Arc<InMemoryChainStore>, Header, Header) {
        let from = make_header(1, 1, None);
        let target = make_header(2, 2, Some(from.hash()));
        let store = Arc::new(InMemoryChainStore::new());
        store.store_header(&from).unwrap();
        store.roll_forward_chain(&from.point()).unwrap();
        store.store_validated_header(&target, &Nonces::for_tests()).unwrap();
        (store, from, target)
    }

    #[tokio::test]
    async fn standalone_reconciliation_repairs_persisted_stores_and_is_idempotent() {
        for chain_ahead in [false, true] {
            let from = make_header(1, 1, None);
            let target = make_header(2, 2, Some(from.hash()));
            let ledger_tip = if chain_ahead { &from } else { &target };
            let test_config = NodeTestConfig::default();
            test_config.chain_store.store_header(ledger_tip).unwrap();
            test_config.chain_store.set_anchor_point(&ledger_tip.point()).unwrap();
            let config = test_config.make_node_configuration().unwrap();
            let ledger_dir = config.ledger_config.ledger_store.dir;
            let directory = tempdir().unwrap();
            let chain_config = RocksDbConfig::new(directory.path().join("chain"));
            {
                let store = RocksDBStore::open_and_migrate(&chain_config).unwrap();
                store.store_validated_header(&from, &Nonces::for_tests()).unwrap();
                store.store_validated_header(&target, &Nonces::for_tests()).unwrap();
                store.roll_forward_chain(&from.point()).unwrap();
                if chain_ahead {
                    store.set_block_valid(&target.hash(), true).unwrap();
                    store.roll_forward_chain(&target.point()).unwrap();
                }
            }

            for expected in [
                StoreRecoveryOutcome::Recovered {
                    before: if chain_ahead { target.point() } else { from.point() },
                    after: ledger_tip.point(),
                },
                StoreRecoveryOutcome::AlreadyConsistent { point: ledger_tip.point() },
            ] {
                assert_eq!(recover_store_pair(&ledger_dir, &chain_config.dir).await.unwrap(), expected);
                let store = RocksDBStore::open(&chain_config).unwrap();
                assert_eq!(store.get_best_chain_tip(), ledger_tip.point());
                assert_eq!(
                    store.load_header_with_validity(&target.hash()).unwrap().1,
                    if chain_ahead { None } else { Some(true) }
                );
                let ledger = RocksDB::new(&RocksDbConfig::new(ledger_dir.clone())).unwrap();
                assert_eq!(NetworkPoint::from(ledger.tip().unwrap()), NetworkPoint::from(ledger_tip.point()));
            }
            acquire_sync_locks([ledger_dir.as_path(), chain_config.dir.as_path()]).unwrap();
        }
    }

    #[tokio::test]
    async fn standalone_reconciliation_respects_synchronization_locks() {
        let directory = tempdir().unwrap();
        let ledger_dir = directory.path().join("ledger");
        let chain_dir = directory.path().join("chain");
        let cache_dir = directory.path().join("cache");
        for path in [&ledger_dir, &chain_dir, &cache_dir] {
            fs::create_dir(path).unwrap();
        }

        for path in [&ledger_dir, &chain_dir] {
            let locks = acquire_sync_locks([path.as_path()]).unwrap();
            let expected = fs::canonicalize(path).unwrap();
            assert!(matches!(
                recover_store_pair(&ledger_dir, &chain_dir).await,
                Err(MithrilSyncError::Concurrent { path: locked }) if locked == expected
            ));
            drop(locks);
            acquire_sync_locks([cache_dir.as_path(), ledger_dir.as_path(), chain_dir.as_path()]).unwrap();
        }
    }

    #[test]
    fn recovery_operational_failures_do_not_recommend_rebootstrap() {
        for chain_ahead in [false, true] {
            for failure in [
                StoreError::WriteError { error: "disk full".to_owned() },
                StoreError::ReadError { error: "I/O failure".to_owned() },
            ] {
                let from = make_header(1, 1, None);
                let target = make_header(2, 2, Some(from.hash()));
                let store = Arc::new(InMemoryChainStore::new());
                store.store_validated_header(&from, &Nonces::for_tests()).unwrap();
                store.store_validated_header(&target, &Nonces::for_tests()).unwrap();
                store.roll_forward_chain(&from.point()).unwrap();
                if chain_ahead {
                    store.roll_forward_chain(&target.point()).unwrap();
                }
                let before = store.get_best_chain_tip();
                let ledger_tip = if chain_ahead { from.point() } else { target.point() };
                let anchor_error = failure.clone();
                let validity_error = failure.clone();
                let failing = OverridingChainStore::builder(store.clone())
                    .with_set_anchor_point(move |_, _| Err(anchor_error.clone()))
                    .with_set_block_valid(move |_, _, _| Err(validity_error.clone()))
                    .build();

                let error = recover_stores(&failing, ledger_tip).unwrap_err();
                assert!(matches!(error, MithrilSyncError::Store { source, .. }
                    if source.downcast_ref::<StoreError>() == Some(&failure)));
                assert_eq!(store.get_best_chain_tip(), before);
            }
        }
    }

    #[test]
    fn unsupported_recovery_does_not_mutate_stores() {
        let parent = make_header(1, 1, None);
        let adopted = make_header(2, 2, Some(parent.hash()));
        let ledger = make_header(2, 3, Some(parent.hash()));
        let store = Arc::new(InMemoryChainStore::new());
        for header in [&parent, &adopted, &ledger] {
            store.store_validated_header(header, &Nonces::for_tests()).unwrap();
        }
        store.roll_forward_chain(&parent.point()).unwrap();
        store.roll_forward_chain(&adopted.point()).unwrap();
        let guarded = OverridingChainStore::builder(store.clone())
            .with_set_anchor_point(|_, _| panic!("unsupported recovery must not change the anchor"))
            .with_set_block_valid(|_, _, _| panic!("unsupported recovery must not change validity"))
            .build();

        assert!(matches!(recover_stores(&guarded, ledger.point()), Err(MithrilSyncError::RebootstrapRequired(_))));
        assert_eq!(store.get_best_chain_tip(), adopted.point());
    }

    #[tokio::test]
    async fn worker_shutdown_failure_takes_precedence_over_ingestion_errors() {
        for ingestion_error in [
            MithrilSyncError::Cancelled,
            MithrilSyncError::InvalidCache { source: anyhow!("invalid block") },
            MithrilSyncError::RebootstrapRequired(Box::new(RebootstrapRequired {
                ledger_tip: Point::Origin,
                chain_tip: Point::Origin,
                reason: "unsupported interrupted state".to_owned(),
            })),
        ] {
            let test_config = NodeTestConfig::default();
            let header = make_header(1, 1, None);
            test_config.chain_store.store_header(&header).unwrap();
            test_config.chain_store.set_anchor_point(&header.point()).unwrap();
            let config = test_config.make_node_configuration().unwrap();
            let idle_references = Arc::strong_count(&test_config.chain_store);
            let state = make_state(&config.ledger_config, None, test_config.chain_store.clone()).unwrap();
            let validator =
                make_block_validator(&config.ledger_config, state, test_config.chain_store.clone()).unwrap();
            let stop = validator.thread_stop();
            let retained = validator.clone();
            let cancellation = MithrilCancellation::new();
            let mut reconciled = false;
            let result = complete_ingestion(
                async move {
                    if matches!(ingestion_error, MithrilSyncError::Cancelled) {
                        cancellation.cancel();
                        cancellation.cancelled().await;
                    }
                    drop(validator);
                    Err::<(), _>(ingestion_error)
                },
                async {
                    stop_ledger_worker(stop, Duration::ZERO).await?;
                    reconciled = true;
                    Ok(())
                },
            )
            .await;

            drop(retained);
            tokio::time::timeout(Duration::from_secs(5), async {
                while Arc::strong_count(&test_config.chain_store) > idle_references {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert!(!reconciled);
            assert!(matches!(result, Err(MithrilSyncError::WorkerShutdown { source })
                if matches!(source.downcast_ref::<amaru_consensus::block_validator::LedgerThreadJoinError>(),
                    Some(amaru_consensus::block_validator::LedgerThreadJoinError::Timeout { .. }))));
        }
    }

    #[tokio::test]
    async fn zero_block_limit_skips_ingestion() {
        let directory = tempdir().unwrap();
        let synchronizer = MithrilSynchronizer::new(
            NetworkName::Preprod,
            directory.path().join("ledger"),
            directory.path().join("chain"),
            directory.path().join("snapshots"),
        )
        .ingest_limits(None, Some(0));
        let tip = make_header(1, 1, None);
        let store = Arc::new(InMemoryChainStore::new());
        store.store_header(&tip).unwrap();
        store.roll_forward_chain(&tip.point()).unwrap();
        let observer = RecordingObserver::default();

        let result = synchronizer
            .ingest(
                store.clone(),
                &directory.path().join("missing-immutable"),
                tip.point(),
                &MithrilCancellation::new(),
                &observer,
            )
            .await
            .unwrap();

        assert_eq!(result, (tip.point(), 0));
        assert_eq!(store.get_best_chain_tip(), tip.point());
        assert!(observer.0.lock().unwrap().is_empty());
        assert!(!synchronizer.ledger_dir.exists());
    }

    #[test]
    fn forwards_download_and_verification_progress_in_order() {
        let observer = Arc::new(RecordingObserver::default());
        let forwarder = ForwardDownloadProgress(observer.clone());
        let stages = [
            (MithrilDownloadStage::FetchingSnapshot, MithrilStage::FetchingSnapshot),
            (MithrilDownloadStage::ValidatingCertificate, MithrilStage::ValidatingCertificate),
            (MithrilDownloadStage::Downloading { files: 6 }, MithrilStage::Downloading),
            (
                MithrilDownloadStage::VerifyingDatabase { from_chunk: 10, through_chunk: 11, files: 6 },
                MithrilStage::VerifyingDatabase { from_chunk: 10, through_chunk: 11, files: 6 },
            ),
            (MithrilDownloadStage::DatabaseVerified, MithrilStage::DatabaseVerified),
        ];
        let mut expected = Vec::new();
        for (download_stage, stage) in stages {
            forwarder.on_progress(MithrilDownloadProgress::StageChanged { stage: download_stage });
            expected.push(MithrilProgress::StageChanged { stage });
            if stage == MithrilStage::ValidatingCertificate {
                for _ in 0..2 {
                    forwarder.on_progress(MithrilDownloadProgress::CertificateValidated);
                    expected.push(MithrilProgress::CertificateValidated);
                }
            }
        }
        assert_eq!(*observer.0.lock().unwrap(), expected);
    }

    #[test_case(true; "dropped_future")]
    #[test_case(false; "cancelled_token")]
    #[tokio::test]
    async fn cancellation_finishes_cleanup_and_releases_locks(abort: bool) {
        let directory = tempdir().unwrap();
        let locks = acquire_sync_locks([directory.path(); 3]).unwrap();
        let (store, from, target) = interrupted_ingestion_store();
        let target_point = target.point();
        let cleanup_store = store.clone();
        let cancellation = MithrilCancellation::new();
        let task_cancellation = cancellation.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (stopping_tx, stopping_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
        let caller = tokio::spawn(run_synchronization(task_cancellation, move |cancellation| async move {
            let (validator, worker_rx) = tokio::sync::oneshot::channel::<()>();
            let worker = tokio::task::spawn_blocking(move || {
                assert!(worker_rx.blocking_recv().is_err());
                target_point
            });
            let ingestion = async move {
                started_tx.send(()).unwrap();
                cancellation.cancelled().await;
                drop(validator);
                Err::<(), _>(MithrilSyncError::Cancelled)
            };
            let cleanup = async move {
                stopping_tx.send(()).unwrap();
                release_rx.await.unwrap();
                let ledger_tip = worker.await.unwrap();
                recover_stores(cleanup_store.as_ref(), ledger_tip).map(|_| ())
            };
            let result = complete_ingestion(ingestion, cleanup).await;
            drop(locks);
            finished_tx.send(()).unwrap();
            result
        }));
        started_rx.await.unwrap();
        if abort {
            caller.abort();
        } else {
            cancellation.cancel();
        }
        tokio::time::timeout(Duration::from_secs(5), stopping_rx).await.unwrap().unwrap();
        assert_eq!(store.get_best_chain_tip(), from.point());
        assert!(matches!(acquire_sync_locks([directory.path(); 3]), Err(MithrilSyncError::Concurrent { .. })));
        if !abort {
            assert!(!caller.is_finished());
        }
        release_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), finished_rx).await.unwrap().unwrap();
        assert_eq!(store.get_best_chain_tip(), target.point());
        acquire_sync_locks([directory.path(); 3]).unwrap();
        if abort {
            assert!(caller.await.unwrap_err().is_cancelled());
            assert!(!cancellation.is_cancelled());
        } else {
            assert!(matches!(caller.await.unwrap(), Err(MithrilSyncError::Cancelled)));
        }
    }

    struct PanickingObserver;

    impl MithrilObserver for PanickingObserver {
        fn on_progress(&self, _: MithrilProgress) {
            panic!("observer failed");
        }
    }

    #[tokio::test]
    async fn observer_panics_join_the_worker_and_reconcile_stores() {
        for progress in [
            MithrilProgress::StageChanged { stage: MithrilStage::Ingesting },
            MithrilProgress::BlocksIngested { blocks: 1, point: Point::Origin },
        ] {
            let (store, _, target) = interrupted_ingestion_store();
            let target_point = target.point();
            let cleanup_store = store.clone();
            let result = run_synchronization(MithrilCancellation::new(), move |_| async move {
                let (validator, worker_rx) = tokio::sync::oneshot::channel::<()>();
                let worker = tokio::task::spawn_blocking(move || {
                    assert!(worker_rx.blocking_recv().is_err());
                    target_point
                });
                let ingestion = async move {
                    PanickingObserver.on_progress(progress);
                    drop(validator);
                    Ok(())
                };
                complete_ingestion(ingestion, async move {
                    let ledger_tip = worker.await.unwrap();
                    recover_stores(cleanup_store.as_ref(), ledger_tip).map(|_| ())
                })
                .await
            });
            let result = tokio::time::timeout(Duration::from_secs(5), result).await.unwrap();
            assert!(matches!(result, Err(MithrilSyncError::TaskFailed { source }) if source.is_panic()));
            assert_eq!(store.get_best_chain_tip(), target.point());
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
        let (store, _, target) = interrupted_ingestion_store();
        let observer = RecordingObserver::default();
        observer.on_progress(MithrilProgress::StageChanged { stage: MithrilStage::ResolvingResumePoint });

        let result = recover_then_download(
            store.as_ref(),
            target.point(),
            Some(NetworkPoint::from(target.point())),
            &MithrilCancellation::new(),
            &observer,
            async {
                assert_eq!(store.get_best_chain_tip(), target.point());
                Err(MithrilDownloadError::Unavailable { source: anyhow!("offline") })
            },
        )
        .await;

        assert!(matches!(result, Err(MithrilSyncError::SnapshotUnavailable { .. })));
        assert_eq!(store.get_best_chain_tip(), target.point());
        assert_eq!(
            observer.0.lock().unwrap().as_slice(),
            [MithrilStage::ResolvingResumePoint, MithrilStage::RecoveringStores, MithrilStage::Downloading]
                .map(|stage| MithrilProgress::StageChanged { stage })
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

        let locks = acquire_sync_locks([&first_cache, &ledger_dir, &chain_dir]).unwrap();
        assert!(directory.path().join(".ledger.mithril-sync.lock").exists());
        assert!(directory.path().join(".chain.mithril-sync.lock").exists());
        assert!(matches!(
            acquire_sync_locks([&second_cache, &ledger_dir, &chain_dir]),
            Err(MithrilSyncError::Concurrent { .. })
        ));
        drop(locks);
        acquire_sync_locks([&second_cache, &ledger_dir, &chain_dir]).unwrap();
    }

    #[derive(Clone, Copy)]
    enum InterruptedAfter {
        BeforeWrites,
        HeaderAndNonces,
        LedgerCommit,
        Validity,
        ChainAdoption,
    }

    #[test_case(InterruptedAfter::BeforeWrites; "before_writes")]
    #[test_case(InterruptedAfter::HeaderAndNonces; "header_and_nonces")]
    #[test_case(InterruptedAfter::LedgerCommit; "ledger_commit")]
    #[test_case(InterruptedAfter::Validity; "validity")]
    #[test_case(InterruptedAfter::ChainAdoption; "chain_adoption")]
    fn recovery_handles_each_cross_store_write_boundary(boundary: InterruptedAfter) {
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
