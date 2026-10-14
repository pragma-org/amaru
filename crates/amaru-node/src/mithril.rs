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
    block_validator::{LedgerThreadJoinError, LedgerThreadStop},
    effects::{
        ConsensusMode, ResourceBlockValidation, ResourceConsensusParameters, ResourceEraHistory, ResourcePoolSummaries,
    },
    performance::Performance,
};
use amaru_kernel::{IsHeader, NetworkName, NetworkPoint, ORIGIN_HASH, Point, RawBlock, Slot};
use amaru_ledger::store::ReadStore;
use amaru_mithril::{
    MithrilDownloadError, MithrilDownloadObserver, MithrilDownloadProgress, MithrilDownloadReport,
    MithrilDownloadStage, download_from_mithril_for_range_with_observer, last_immutable_point, read_blocks_after_point,
};
use amaru_observability::info;
use amaru_ouroboros::ChainStore;
use amaru_progress_bar::{ProgressBar, ProgressBarExt, TerminalProgressBar};
use amaru_protocols::store_effects::ResourceHeaderStore;
use amaru_pure_stage::{
    StageGraph, StageGraphRunning,
    tokio::{TokioBuilder, TokioRunning},
};
use amaru_stores::rocksdb::{ReadOnlyRocksDB, RocksDbConfig};
use anyhow::anyhow;
use futures_util::{FutureExt, StreamExt};
use same_file::Handle;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::{
    ClearValidity, ComponentFailure, NodeStartError, ShutdownError, ShutdownReport,
    chain_realign::ensure_store_consistency,
    realign_chain_store_to,
    stages::{
        build_node::{StoreOpenOperation, ledger_store_error, make_block_validator, make_state, open_chain_store},
        config::{Config, LedgerConfig},
    },
};

mod block_source;
use block_source::{MithrilBlockEvent, MithrilInput, wire_mithril_stages};

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
    /// Number of blocks expected from the downloaded immutable database after the resume point.
    IngestPlanned {
        total_blocks: u64,
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
    downloaded_bytes: Option<u64>,
    verification_phase: Option<&'static str>,
}

impl StructuredRenderer {
    fn render(&mut self, progress: MithrilProgress) {
        match &progress {
            MithrilProgress::Downloaded { downloaded_bytes, .. } => {
                self.downloaded_bytes = Some(*downloaded_bytes);
            }
            MithrilProgress::StageChanged { stage } => {
                if let Some(phase) = self.verification_phase.take() {
                    info!(mithril::progress::PHASE_COMPLETED, phase);
                }
                if *stage != MithrilStage::Downloading {
                    self.finish_download();
                }
                self.verification_phase = match stage {
                    MithrilStage::ValidatingCertificate | MithrilStage::VerifyingDatabase { .. } => {
                        Some(stage.as_str())
                    }
                    MithrilStage::ResolvingResumePoint
                    | MithrilStage::FetchingSnapshot
                    | MithrilStage::Downloading
                    | MithrilStage::DatabaseVerified
                    | MithrilStage::RecoveringStores
                    | MithrilStage::Ingesting => None,
                };
            }
            MithrilProgress::Completed { .. } => self.finish_download(),
            MithrilProgress::SnapshotSelected { .. }
            | MithrilProgress::CertificateValidated
            | MithrilProgress::IngestPlanned { .. }
            | MithrilProgress::BlocksIngested { .. } => {}
        }
        if self.should_render(&progress, Instant::now()) {
            render_structured(progress);
        }
    }

    fn finish_download(&mut self) {
        if let Some(downloaded_bytes) = self.downloaded_bytes.take() {
            info!(mithril::progress::PHASE_COMPLETED, phase = MithrilStage::Downloading.as_str(), downloaded_bytes);
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
            | MithrilProgress::IngestPlanned { .. }
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
    ingested_blocks: u64,
    planned_blocks: u64,
    total_bytes: Option<u64>,
    download: Option<Box<dyn ProgressBar>>,
    verification: Option<(&'static str, Box<dyn ProgressBar>)>,
}

impl TerminalRenderer {
    fn render(&mut self, progress: MithrilProgress) {
        if matches!(progress, MithrilProgress::StageChanged { .. } | MithrilProgress::Completed { .. }) {
            if let Some((phase, verification)) = self.verification.take() {
                verification.finish(|| {
                    info!(mithril::progress::PHASE_COMPLETED, phase);
                });
            }
            if !matches!(progress, MithrilProgress::StageChanged { stage: MithrilStage::Downloading })
                && let Some(download) = self.download.take()
            {
                download.finish(|| {
                    info!(
                        mithril::progress::PHASE_COMPLETED,
                        phase = MithrilStage::Downloading.as_str(),
                        downloaded_bytes = self.downloaded_bytes
                    );
                });
            }
        }
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
                if stage == MithrilStage::Ingesting {
                    self.ingested_blocks = 0;
                }
                let template = match stage {
                    MithrilStage::ValidatingCertificate => Some(
                        "{spinner:.green} {elapsed_precise} validating Mithril certificate chain ({pos} certificates)",
                    ),
                    MithrilStage::VerifyingDatabase { .. } => {
                        Some("{spinner:.green} {elapsed_precise} verifying Mithril database")
                    }
                    MithrilStage::Ingesting if self.planned_blocks > 0 => {
                        Some("{spinner:.green} Ingesting {pos}/{len} blocks {wide_bar:.green} {per_sec} ETA {eta}")
                    }
                    MithrilStage::Ingesting => Some("{spinner:.green} Ingesting {pos} blocks ({per_sec})"),
                    MithrilStage::ResolvingResumePoint
                    | MithrilStage::FetchingSnapshot
                    | MithrilStage::Downloading
                    | MithrilStage::DatabaseVerified
                    | MithrilStage::RecoveringStores => None,
                };
                let length = if stage == MithrilStage::Ingesting { self.planned_blocks } else { 0 };
                self.verification =
                    template.map(|template| (stage.as_str(), TerminalProgressBar::new(length, template).boxed()));
            }
            MithrilProgress::IngestPlanned { total_blocks } => self.planned_blocks = total_blocks,
            MithrilProgress::CertificateValidated => {
                if let Some((_, verification)) = &self.verification {
                    verification.increment();
                }
            }
            MithrilProgress::BlocksIngested { blocks, .. } => {
                if let Some((_, verification)) = &self.verification {
                    verification
                        .tick(usize::try_from(blocks.saturating_sub(self.ingested_blocks)).unwrap_or(usize::MAX));
                }
                self.ingested_blocks = blocks;
            }
            MithrilProgress::SnapshotSelected { .. } | MithrilProgress::Completed { .. } => {}
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
        MithrilProgress::IngestPlanned { .. } => {}
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
        let (source, mut events, running, ledger_thread, performance) = {
            let ledger_config = ledger_config_for_network(self.network, self.ledger_dir.clone())?;
            let config = Config { ledger_config, ..Config::default() };
            let mut stage_graph =
                TokioBuilder::default().with_global_epoch_offset(config.compute_global_clock_offset());
            let ledger_config = &config.ledger_config;
            let state = make_state(ledger_config, None, chain_store.clone())
                .map_err(|source| MithrilSyncError::Startup(source.into()))?;
            let ledger_tip = state.tip().into_owned();
            if NetworkPoint::from(ledger_tip) != NetworkPoint::from(resume_point) {
                return Err(MithrilSyncError::ResumePointMismatch {
                    requested: resume_point.into(),
                    stored: ledger_tip.into(),
                });
            }
            let security_param = ledger_config.global_parameters.consensus_security_param;
            advance_replay_anchor(chain_store.as_ref(), ledger_tip, security_param)?;
            let ledger_tip = resolve_resume_point(chain_store.as_ref(), ledger_tip.into())
                .map_err(|_| MithrilSyncError::Startup(anyhow!("ledger tip header not found").into()))?;
            let pool_summaries = state.pool_summaries();
            let validator = Arc::new(
                make_block_validator(ledger_config, state, chain_store.clone())
                    .map_err(|source| MithrilSyncError::Startup(source.into()))?,
            );
            stage_graph.resources().put::<ResourceHeaderStore>(chain_store.clone());
            stage_graph.resources().put::<ResourcePoolSummaries>(Arc::new(pool_summaries));
            stage_graph.resources().put::<ResourceBlockValidation>(validator.clone());
            stage_graph
                .resources()
                .put::<ResourceConsensusParameters>(Arc::new(ledger_config.to_consensus_parameters()));
            stage_graph.resources().put::<ResourceEraHistory>(ledger_config.era_history.clone());
            stage_graph.resources().put(ConsensusMode::Sync);
            let ledger_thread = validator.thread_stop();
            let performance = Performance::new().install(stage_graph.resources());
            let (source, events) =
                wire_mithril_stages(&mut stage_graph, ledger_tip, security_param, self.ingest_until_slot);
            // The validator lives in these resources; a strong capture would keep the stores open.
            let resources = stage_graph.resources().downgrade();
            let stake_updates = source.clone();
            validator.set_on_stake_dist_updated(Arc::new(move |summaries| {
                let max_epoch = summaries.max_epoch();
                resources.put::<ResourcePoolSummaries>(Arc::new(summaries));
                let send = async {
                    if stake_updates.send(MithrilInput::StakeDistUpdated(max_epoch)).await.is_err() {
                        amaru_observability::warn!(node::build::STAKE_DIST_NOTIFY_FAILED);
                    }
                };
                // This runs on the ledger thread; its join also covers notification delivery.
                #[expect(clippy::expect_used)]
                let rt =
                    tokio::runtime::Builder::new_current_thread().build().expect("cannot build current thread runtime");
                rt.block_on(send);
            }));
            let running = stage_graph.run(tokio::runtime::Handle::current());
            (source, events, running, ledger_thread, performance)
        };
        let mut termination = running.termination();

        let before = Instant::now();
        let ingestion = async move {
            let last_point = last_immutable_point(immutable_dir, self.ingest_until_slot)
                .map_err(|source| MithrilSyncError::InvalidCache { source })?;
            let total_blocks = last_point
                .map(|point| point.block_height() - resume_point.block_height())
                .unwrap_or(0)
                .min(self.ingest_maximum_blocks.unwrap_or(usize::MAX) as u64);
            observer.on_progress(MithrilProgress::IngestPlanned { total_blocks });
            observer.on_progress(MithrilProgress::StageChanged { stage: MithrilStage::Ingesting });
            let blocks = read_blocks_after_point(immutable_dir, self.network, resume_point)
                .map_err(|source| MithrilSyncError::InvalidCache { source })?;
            let mut processed = 0_u64;
            for raw_block in blocks.take(self.ingest_maximum_blocks.unwrap_or(usize::MAX)) {
                if cancellation.is_cancelled() {
                    return Err(MithrilSyncError::Cancelled);
                }
                let raw_block = RawBlock::from(
                    raw_block.map_err(|source| MithrilSyncError::InvalidCache { source })?.into_boxed_slice(),
                );
                tokio::select! {
                    result = source.send(MithrilInput::Block(raw_block)) => {
                        result.map_err(|source| store_error("send block to Mithril stage", source))?;
                    }
                    _ = cancellation.cancelled() => return Err(MithrilSyncError::Cancelled),
                }
                let event = tokio::select! {
                    event = events.next() => event.ok_or_else(|| store_error("receive Mithril stage result", anyhow!("stage output closed")))?,
                    _ = &mut termination => return Err(store_error("run Mithril stages", anyhow!("stage graph terminated"))),
                    _ = cancellation.cancelled() => return Err(MithrilSyncError::Cancelled),
                };
                let point = match event {
                    MithrilBlockEvent::Applied(point) => point,
                    MithrilBlockEvent::Finished => break,
                    MithrilBlockEvent::Failed(point, reason) => {
                        return Err(MithrilSyncError::Validation { point, source: anyhow!(reason) });
                    }
                };
                processed += 1;
                observer.on_progress(MithrilProgress::BlocksIngested { blocks: processed, point });
            }
            if cancellation.is_cancelled() {
                return Err(MithrilSyncError::Cancelled);
            }
            Ok(processed)
        };
        let cleanup = async {
            let stage_report = shutdown_replay(running, ledger_thread, performance, Duration::from_secs(10))
                .await
                .map_err(|source| MithrilSyncError::WorkerShutdown { source: source.into() })?;

            if let Some(failure) = stage_report
                .unexpected_exits
                .iter()
                .find(|failure| matches!(failure, crate::ComponentFailure::Ledger(_)))
            {
                return Err(MithrilSyncError::WorkerShutdown { source: anyhow!("{failure:?}") });
            }
            let ledger_tip = resolve_ledger_tip(&self.ledger_dir, chain_store.as_ref())?;
            let point = recover_stores(chain_store.as_ref(), ledger_tip).map(StoreRecoveryOutcome::point)?;
            if !stage_report.unexpected_exits.is_empty() {
                return Err(store_error(
                    "join Mithril stages",
                    anyhow!("stages exited before shutdown: {:?}", stage_report.unexpected_exits),
                ));
            }
            Ok(point)
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

/// Stop stages before joining their workers and releasing the stores.
async fn shutdown_replay(
    tokio_running: TokioRunning,
    ledger_thread: LedgerThreadStop,
    performance: impl FnOnce() -> std::thread::Result<()> + Send + 'static,
    timeout: Duration,
) -> Result<ShutdownReport, ShutdownError> {
    tokio_running.request_abort();
    let mut unexpected_exits = match tokio_running.join().await {
        Ok(report) => {
            report.unexpected_exits.into_iter().map(|stage| ComponentFailure::StageExited { stage }).collect()
        }
        Err(error) => vec![ComponentFailure::Stage(error.to_string())],
    };
    let (ledger, performance) =
        tokio::task::spawn_blocking(move || (ledger_thread.join_timeout(timeout), performance()))
            .await
            .map_err(|error| ShutdownError::JoinTask { reason: error.to_string() })?;
    let performance = performance.err().map(|_| ComponentFailure::Performance("worker panicked".into()));
    let ledger = match ledger {
        Ok(()) => None,
        Err(LedgerThreadJoinError::Panicked(message)) => Some(ComponentFailure::Ledger(message)),
        Err(LedgerThreadJoinError::Timeout { timeout }) => return Err(ShutdownError::LedgerTimeout { timeout }),
    };

    unexpected_exits.extend([ledger, performance].into_iter().flatten());
    Ok(ShutdownReport { unexpected_exits })
}

/// Drop the ingestion future before joining the worker, even on panic.
/// Return the reconciled store tip with the ingestion block count.
async fn complete_ingestion(
    ingestion: impl Future<Output = Result<u64, MithrilSyncError>>,
    cleanup: impl Future<Output = Result<Point, MithrilSyncError>>,
) -> Result<(Point, u64), MithrilSyncError> {
    let result = AssertUnwindSafe(ingestion).catch_unwind().await;
    let final_point = cleanup.await?;
    match result {
        Ok(result) => result.map(|processed| (final_point, processed)),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

fn resolve_ledger_tip(ledger_dir: &Path, chain_store: &dyn ChainStore) -> Result<Point, MithrilSyncError> {
    let ledger = ReadOnlyRocksDB::new(&RocksDbConfig::new(ledger_dir.to_path_buf()))
        .map_err(|source| MithrilSyncError::Startup(ledger_store_error(source, StoreOpenOperation::LedgerReadOnly)))?;
    let stored = NetworkPoint::from(ledger.tip().map_err(|source| store_error("read ledger tip", source))?);
    resolve_resume_point(chain_store, stored)
}

fn resolve_resume_point(chain_store: &dyn ChainStore, stored: NetworkPoint) -> Result<Point, MithrilSyncError> {
    chain_store
        .load_point(&stored.hash())
        .filter(|point| NetworkPoint::from(*point) == stored)
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

struct SyncLock {
    file: File,
    path: PathBuf,
}

impl Drop for SyncLock {
    fn drop(&mut self) {
        if matches!(lock_points_to_path(&self.file, &self.path), Ok(true)) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn acquire_sync_locks<const N: usize>(directories: [&Path; N]) -> Result<Vec<SyncLock>, MithrilSyncError> {
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
            let path = sync_lock_path(directory)?;
            let file = File::create(&path).map_err(|source| store_error("create synchronization lock", source))?;
            lock_sync_file(&file, &path, directory)?;
            Ok(SyncLock { file, path })
        })
        .collect()
}

/// Detect a lock file replaced between opening it and acquiring its lock.
fn lock_points_to_path(file: &File, path: &Path) -> io::Result<bool> {
    let locked = Handle::from_file(file.try_clone()?)?;
    let current = match Handle::from_path(path) {
        Ok(current) => current,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(source) => return Err(source),
    };
    Ok(locked == current)
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

fn lock_sync_file(lock: &File, path: &Path, directory: &Path) -> Result<(), MithrilSyncError> {
    match lock.try_lock() {
        Ok(()) => {
            if lock_points_to_path(lock, path).map_err(|source| store_error("verify synchronization lock", source))? {
                Ok(())
            } else {
                Err(MithrilSyncError::Concurrent { path: directory.to_path_buf() })
            }
        }
        Err(TryLockError::WouldBlock) => Err(MithrilSyncError::Concurrent { path: directory.to_path_buf() }),
        Err(source) => Err(store_error("acquire synchronization lock", source)),
    }
}

fn recover_stores(chain_store: &dyn ChainStore, ledger_tip: Point) -> Result<StoreRecoveryOutcome, MithrilSyncError> {
    let chain_tip = chain_store.get_best_chain_tip();
    if chain_tip == ledger_tip {
        return Ok(StoreRecoveryOutcome::AlreadyConsistent { point: ledger_tip });
    }
    let can_adopt = chain_store.load_header_with_validity(&ledger_tip.hash()).is_some_and(|(header, validity)| {
        header.point() == ledger_tip
            && validity != Some(false)
            && header.parent_hash().unwrap_or(ORIGIN_HASH) == chain_tip.hash()
            && chain_store.get_nonces(&ledger_tip.hash()).is_some()
    });
    if can_adopt {
        adopt_validated_block(chain_store, ledger_tip)
            .map_err(|source| store_error("adopt recovered ledger tip", source))?;
        info!(cli::mithril::RECOVER_CHAIN_TIP, ledger_tip, chain_tip);
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

fn adopt_validated_block(chain_store: &dyn ChainStore, point: Point) -> anyhow::Result<()> {
    chain_store.set_block_valid(&point.hash(), true)?;
    chain_store.roll_forward_chain(&point)?;
    let chain_tip = chain_store.get_best_chain_tip();
    if chain_tip != point {
        anyhow::bail!("adopted chain tip {chain_tip} does not match ledger tip {point}");
    }
    Ok(())
}

/// Catch up an old replay anchor once, walking backward from the tip by the security parameter.
/// Subsequent adoptions advance it through the regular consensus stage.
fn advance_replay_anchor(
    chain_store: &dyn ChainStore,
    tip: Point,
    security_param: u64,
) -> Result<(), MithrilSyncError> {
    let snapshot = chain_store.snapshot();
    if snapshot.get_best_chain_tip() != tip {
        return Err(store_error("find replay anchor", anyhow!("{tip} is not the adopted best-chain tip")));
    }
    let target_height = tip.block_height() - security_param;
    if target_height <= snapshot.get_anchor_point().block_height() {
        return Ok(());
    }
    let anchor =
        std::iter::successors(snapshot.load_header(&tip.hash()), |header| snapshot.load_header(&header.parent()?))
            .find(|header| header.block_height() <= target_height)
            .ok_or_else(|| {
                store_error("find replay anchor", anyhow!("missing best-chain ancestor at height {target_height}"))
            })?
            .point();
    drop(snapshot);
    chain_store.set_anchor_point(&anchor).map_err(|source| store_error("advance replay anchor", source))?;
    Ok(())
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
    use std::{fs::File, sync::Mutex, time::Duration};

    use amaru_consensus::{
        effects::{ConsensusMode, ResourceBlockValidation, ResourceEraHistory},
        performance::{Performance, ResourcePerformance},
    };
    use amaru_kernel::{Epoch, EraHistory, Header, IsHeader, cardano::network_block::EncodedTestBlock, make_header};
    use amaru_ledger::store::ReadStore;
    use amaru_ouroboros::{
        BaseReadChainStore, MockBlockValidator, Nonces, StoreError, WriteChainStore,
        in_memory_chain_store::InMemoryChainStore, overriding_chain_store::OverridingChainStore,
    };
    use amaru_protocols::store_effects::ResourceHeaderStore;
    use amaru_pure_stage::{StageGraph, tokio::TokioBuilder};
    use amaru_stores::rocksdb::{RocksDB, consensus::RocksDBStore};
    use tempfile::tempdir;
    use test_case::test_case;

    use super::{
        block_source::{MithrilBlockSource, mithril_block_source},
        *,
    };
    use crate::{
        stages::build_node::{make_block_validator, make_state},
        tests::configuration::NodeTestConfig,
    };

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

    #[test]
    fn replay_anchor_catches_up_to_the_stable_horizon() {
        let store = InMemoryChainStore::new();
        let mut parent = None;
        let mut points = Vec::new();
        for height in 1..=8 {
            let header = make_header(height, height, parent);
            parent = Some(header.hash());
            store.store_header(&header).unwrap();
            store.roll_forward_chain(&header.point()).unwrap();
            points.push(header.point());
        }
        store.set_anchor_point(&points[0]).unwrap();
        advance_replay_anchor(&store, points[7], 8).unwrap();
        assert_eq!(store.get_anchor_point(), points[0]);

        for security_param in [2, 2, 4] {
            advance_replay_anchor(&store, points[7], security_param).unwrap();
            assert_eq!(store.get_anchor_point(), points[5]);
        }
        assert!(advance_replay_anchor(&store, points[6], 2).is_err());
        assert_eq!(store.get_anchor_point(), points[5]);
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
                Arc::new(make_block_validator(&config.ledger_config, state, test_config.chain_store.clone()).unwrap());
            let ledger_thread = validator.thread_stop();
            let running = TokioBuilder::default().run(tokio::runtime::Handle::current());
            let retained = validator.clone();
            let mut reconciled = false;
            let result = complete_ingestion(
                async move {
                    drop(validator);
                    Err::<u64, _>(ingestion_error)
                },
                async {
                    let report = shutdown_replay(running, ledger_thread, || Ok(()), Duration::ZERO)
                        .await
                        .map_err(|source| MithrilSyncError::WorkerShutdown { source: source.into() })?;
                    assert!(report.is_clean());
                    reconciled = true;
                    Ok(Point::Origin)
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
                    if matches!(source.downcast_ref::<crate::ShutdownError>(),
                        Some(crate::ShutdownError::LedgerTimeout { .. }))));
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

    #[test_case(false; "valid_block")]
    #[test_case(true; "rejected_block")]
    #[tokio::test]
    async fn mithril_blocks_use_the_shared_validation_and_adoption_stages(reject: bool) {
        let parent = make_header(1, 1, None);
        let block = EncodedTestBlock::from_seed(&make_header(2, 2, Some(parent.hash())), &EraHistory::default());
        let store = Arc::new(InMemoryChainStore::new());
        store.store_validated_header(&parent, &Nonces::for_tests()).unwrap();
        store.store_validated_header(&block.header, &Nonces::for_tests()).unwrap();
        store.roll_forward_chain(&parent.point()).unwrap();
        store.set_anchor_point(&parent.point()).unwrap();

        let mut builder = TokioBuilder::default();
        builder.resources().put::<ResourceHeaderStore>(store.clone());
        let validator = Arc::new(MockBlockValidator::new(parent.point()));
        if reject {
            validator.with_validate_fails(block.header.point());
        }
        builder.resources().put::<ResourceBlockValidation>(validator);
        builder.resources().put::<ResourcePerformance>(Arc::new(Performance::new()));
        builder.resources().put::<ResourceEraHistory>(EraHistory::default());
        builder.resources().put(ConsensusMode::Sync);
        let (source, mut events) = wire_mithril_stages(&mut builder, parent.point(), 2, None);
        let running = builder.run(tokio::runtime::Handle::current());
        source.send(MithrilInput::Block(block.raw)).await.unwrap();
        let event = tokio::time::timeout(Duration::from_secs(5), events.next()).await.unwrap().unwrap();
        match event {
            MithrilBlockEvent::Applied(point) => {
                assert!(!reject);
                assert_eq!(point, block.header.point());
            }
            MithrilBlockEvent::Failed(point, _) => {
                assert!(reject);
                assert_eq!(point, block.header.point());
            }
            MithrilBlockEvent::Finished => panic!("unexpected Mithril stage event"),
        }
        assert_eq!(store.load_header_with_validity(&block.header.hash()).unwrap().1, Some(!reject));
        assert_eq!(store.get_best_chain_tip(), if reject { parent.point() } else { block.header.point() });

        running.request_abort();
        assert!(running.join().await.unwrap().unexpected_exits.is_empty());
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
                Err::<u64, _>(MithrilSyncError::Cancelled)
            };
            let cleanup = async move {
                stopping_tx.send(()).unwrap();
                release_rx.await.unwrap();
                let ledger_tip = worker.await.unwrap();
                recover_stores(cleanup_store.as_ref(), ledger_tip).map(StoreRecoveryOutcome::point)
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
                    Ok(0)
                };
                complete_ingestion(ingestion, async move {
                    let ledger_tip = worker.await.unwrap();
                    recover_stores(cleanup_store.as_ref(), ledger_tip).map(StoreRecoveryOutcome::point)
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
        for path in [&first_cache, &ledger_dir, &chain_dir] {
            assert!(!sync_lock_path(path).unwrap().exists());
        }
        let locks = acquire_sync_locks([&second_cache, &ledger_dir, &chain_dir]).unwrap();
        drop(locks);
        for path in [&second_cache, &ledger_dir, &chain_dir] {
            assert!(!sync_lock_path(path).unwrap().exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn replaced_lock_file_is_not_accepted() {
        let directory = tempdir().unwrap();
        let path = sync_lock_path(directory.path()).unwrap();
        let stale = File::create(&path).unwrap();
        fs::remove_file(&path).unwrap();
        File::create(&path).unwrap();

        assert!(matches!(lock_sync_file(&stale, &path, directory.path()), Err(MithrilSyncError::Concurrent { .. })));
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

    #[tokio::test]
    async fn replay_retries_a_pending_block_on_stake_distribution_update() {
        let target = Epoch::from(1120);
        let parent = make_header(1, 1, None);
        let block = EncodedTestBlock::from_seed(&make_header(2, 2, Some(parent.hash())), &EraHistory::default());
        let store = Arc::new(InMemoryChainStore::new());
        store.store_validated_header(&block.header, &Nonces::for_tests()).unwrap();
        let mut builder = TokioBuilder::default();
        builder.resources().put::<ResourceHeaderStore>(store.clone());
        let (validate_block, mut validations) = builder.output("validate", 1);
        let (events, _) = builder.output("events", 1);
        let stage = builder.stage("source", mithril_block_source);
        let stage = builder.wire_up(
            stage,
            MithrilBlockSource {
                validate_block,
                events,
                tip: parent.point(),
                until_slot: None,
                pending: Some((target, block.raw)),
            },
        );
        let sender = builder.input(stage);
        let running = builder.run(tokio::runtime::Handle::current());
        sender.send(MithrilInput::StakeDistUpdated(target)).await.unwrap();
        let message = tokio::time::timeout(Duration::from_secs(5), validations.next()).await.unwrap().unwrap();
        assert_eq!(
            message,
            amaru_consensus::stages::validate_block::ValidateBlockMsg::new(
                block.header.point(),
                parent.point(),
                block.header.point().block_height(),
            )
        );
        running.request_abort();
        assert!(running.join().await.unwrap().unexpected_exits.is_empty());
    }

    #[test_case("parent"; "wrong_parent")]
    #[test_case("height"; "skipped_height")]
    #[test_case("cbor"; "malformed_block")]
    #[test_case("limit"; "slot_limit")]
    #[test_case("origin"; "origin_replay")]
    #[tokio::test]
    async fn replay_checks_input_before_dispatching_validation(case: &str) {
        let parent = make_header(1, 1, None);
        let tip = if case == "origin" { Point::Origin } else { parent.point() };
        let header = if case == "origin" {
            parent
        } else {
            make_header(
                if case == "height" { 3 } else { 2 },
                2,
                Some(if case == "parent" { Point::Origin.hash() } else { parent.hash() }),
            )
        };
        let block = EncodedTestBlock::from_seed(&header, &EraHistory::default());
        let raw = if case == "cbor" { RawBlock::from(vec![0xff].into_boxed_slice()) } else { block.raw };
        let mut builder = TokioBuilder::default();
        let (source, mut events) =
            wire_mithril_stages(&mut builder, tip, 2, (case == "limit").then_some(Slot::from(1)));
        let running = builder.run(tokio::runtime::Handle::current());
        source.send(MithrilInput::Block(raw)).await.unwrap();
        let event = tokio::time::timeout(Duration::from_secs(5), events.next()).await.unwrap().unwrap();
        if case == "limit" {
            assert_eq!(event, MithrilBlockEvent::Finished);
        } else if case == "origin" {
            assert_eq!(event, MithrilBlockEvent::Failed(header.point(), "replay from origin is not supported".into()));
        } else {
            assert!(matches!(event, MithrilBlockEvent::Failed(..)), "{event:?}");
        }
        running.request_abort();
        assert!(running.join().await.unwrap().unexpected_exits.is_empty());
    }

    #[tokio::test]
    async fn failed_ingestion_releases_existing_rocksdb_stores() {
        let test_config = NodeTestConfig::default();
        let config = test_config.make_node_configuration().unwrap();
        let directory = tempdir().unwrap();
        let chain_config = RocksDbConfig::new(directory.path().join("chain"));
        let chain_store = Arc::new(RocksDBStore::open_and_migrate(&chain_config).unwrap());
        let synchronizer = MithrilSynchronizer::new(
            config.network(),
            config.ledger_config.ledger_store.dir.clone(),
            chain_config.dir.clone(),
            directory.path().join("snapshots"),
        );
        let result = synchronizer
            .ingest(
                chain_store.clone(),
                &directory.path().join("missing-immutable"),
                Point::Origin,
                &MithrilCancellation::new(),
                &RecordingObserver::default(),
            )
            .await;
        assert!(matches!(result, Err(MithrilSyncError::InvalidCache { .. })), "{result:?}");
        assert_eq!(Arc::strong_count(&chain_store), 1);
        drop(chain_store);
        drop(RocksDBStore::open(&chain_config).unwrap());
        drop(RocksDB::new(&config.ledger_config.ledger_store).unwrap());
    }
}
