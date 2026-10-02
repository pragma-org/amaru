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
    fs,
    future::Future,
    panic::AssertUnwindSafe,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use amaru_kernel::{NetworkName, NetworkPoint, Point, RawBlock, Slot};
use amaru_mithril::{
    MithrilDownloadError, MithrilDownloadObserver, MithrilDownloadProgress, MithrilDownloadReport,
    MithrilDownloadStage, download_from_mithril_for_range_with_observer, last_immutable_point, read_blocks_after_point,
};
use amaru_observability::info;
use amaru_ouroboros::ChainStore;
use amaru_stores::rocksdb::RocksDbConfig;
use anyhow::anyhow;
use futures_util::{FutureExt, StreamExt};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use super::{MithrilBlockEvent, MithrilInput};
use crate::{
    NodeStartError,
    stages::{
        build_node::{launch_stages, on_stake_dist_updated, open_chain_store, prepare_node},
        build_stage_graph::wire_mithril_stages,
        config::{Config, LedgerConfig, StoreType},
    },
};

mod progress;
mod recovery;
pub use progress::{DefaultMithrilObserver, MithrilObserver, MithrilProgress, MithrilStage, MithrilSyncReport};
pub use recovery::{RebootstrapRequired, StoreRecoveryOutcome, reconcile_mithril_stores, recover_store_pair};
use recovery::{
    acquire_sync_locks, advance_replay_anchor, recover_stores, resolve_ledger_tip, validate_store_directories,
};

/// Cooperative cancellation for [`MithrilSynchronizer::synchronize`].
///
/// Cancel the token, then await synchronization completion and inspect its result.
/// [`MithrilSyncError::WorkerShutdown`] requires a process restart before using the stores again.
pub type MithrilCancellation = CancellationToken;

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
        let security_param = ledger_config.global_parameters.consensus_security_param;
        let config =
            Config { ledger_config, chain_store: StoreType::Existing(chain_store.clone()), ..Config::default() };
        let ((source, mut events), running) =
            launch_stages(&config, &tokio::runtime::Handle::current(), |stage_builder| {
                let (node, ()) = prepare_node(&config, stage_builder, None, |chain_store, stable_tip| {
                    if NetworkPoint::from(stable_tip) != NetworkPoint::from(resume_point) {
                        return Err(MithrilSyncError::ResumePointMismatch {
                            requested: resume_point.into(),
                            stored: stable_tip.into(),
                        }
                        .into());
                    }
                    advance_replay_anchor(chain_store, stable_tip, security_param).map_err(Into::into)
                })?;
                let (source, events) =
                    wire_mithril_stages(stage_builder, node.ledger_tip, security_param, self.ingest_until_slot);
                on_stake_dist_updated(
                    stage_builder,
                    &node.block_validator,
                    source.clone(),
                    MithrilInput::StakeDistUpdated,
                );
                Ok((source, events))
            })
            .map_err(|source| {
                source.downcast::<MithrilSyncError>().unwrap_or_else(|source| MithrilSyncError::Startup(source.into()))
            })?;
        let stable_tip = resume_point;
        let mut termination = running.termination();

        let before = Instant::now();
        let ingestion = async move {
            let last_point = last_immutable_point(immutable_dir, self.ingest_until_slot)
                .map_err(|source| MithrilSyncError::InvalidCache { source })?;
            let total_blocks = last_point
                .map(|point| point.block_height() - stable_tip.block_height())
                .unwrap_or(0)
                .min(self.ingest_maximum_blocks.unwrap_or(usize::MAX) as u64);
            observer.on_progress(MithrilProgress::IngestPlanned { total_blocks });
            observer.on_progress(MithrilProgress::StageChanged { stage: MithrilStage::Ingesting });
            let mut current_tip = stable_tip;
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
            let stage_report = running
                .shutdown()
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

/// Drop the ingestion future before joining the worker, even on panic.
/// Return the reconciled store tip with the ingestion block count.
async fn complete_ingestion(
    ingestion: impl Future<Output = Result<(Point, u64), MithrilSyncError>>,
    cleanup: impl Future<Output = Result<Point, MithrilSyncError>>,
) -> Result<(Point, u64), MithrilSyncError> {
    let result = AssertUnwindSafe(ingestion).catch_unwind().await;
    let final_point = cleanup.await?;
    match result {
        Ok(result) => result.map(|(_, processed)| (final_point, processed)),
        Err(payload) => std::panic::resume_unwind(payload),
    }
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
mod tests;
