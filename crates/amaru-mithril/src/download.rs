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
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex as StdMutex},
};

use amaru_kernel::{NetworkName, Point};
use amaru_observability::{info, warn};
use amaru_progress_bar::{NoProgressBar, ProgressBar};
use anyhow::anyhow;
use async_trait::async_trait;
use mithril_client::{
    ClientBuilder, GenesisVerificationKey, MessageBuilder,
    cardano_database_client::{DownloadUnpackOptions, ImmutableFileRange},
    feedback::{FeedbackReceiver, MithrilEvent, MithrilEventCardanoDatabase},
};
use thiserror::Error;
use tokio::sync::Mutex;

use crate::immutable::{chunk_for_slot, validate_immutable_resume_point, validated_download_boundary};

type ProgressFactory = Arc<dyn Fn(usize, &str) -> Box<dyn ProgressBar + Send + Sync> + Send + Sync>;

/// Renderer-independent progress produced by a Mithril database download.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MithrilDownloadProgress {
    SnapshotSelected { hash: String, through_chunk: u64 },
    Downloaded { downloaded_bytes: u64, completed_files: u64, total_files: u64, total_bytes: Option<u64> },
}

/// Passive consumer of canonical Mithril download progress.
pub trait MithrilDownloadObserver: Send + Sync {
    fn on_progress(&self, progress: MithrilDownloadProgress);
}

/// Result of a verified resume-aware Mithril download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MithrilDownloadReport {
    pub immutable_dir: PathBuf,
    /// Downloaded snapshot, or `None` when a verified immutable cache was reused.
    pub snapshot_hash: Option<String>,
}

/// Actionable outcomes from snapshot selection, download, cache validation, and verification.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum MithrilDownloadError {
    #[error("no applicable Mithril snapshot is available")]
    Unavailable {
        #[source]
        source: anyhow::Error,
    },
    #[error("latest Mithril snapshot ends at chunk {through_chunk}, before required chunk {required_chunk}")]
    Inapplicable { through_chunk: u64, required_chunk: u64 },
    #[error("immutable cache validation failed after bounded repair")]
    InvalidCache {
        #[source]
        source: anyhow::Error,
    },
    #[error("Mithril certificate or database verification failed")]
    Validation {
        #[source]
        source: anyhow::Error,
    },
    #[error("Mithril download failed: {0}")]
    Download(#[from] anyhow::Error),
}

#[derive(Default)]
struct MonotonicDownloadState {
    downloaded_bytes: u64,
    completed_files: u64,
    total_files: u64,
    total_bytes: Option<u64>,
}

struct MonotonicDownloadObserver {
    inner: Arc<dyn MithrilDownloadObserver>,
    state: StdMutex<MonotonicDownloadState>,
}

impl MonotonicDownloadObserver {
    fn new(inner: Arc<dyn MithrilDownloadObserver>) -> Self {
        Self { inner, state: StdMutex::new(MonotonicDownloadState::default()) }
    }
}

impl MithrilDownloadObserver for MonotonicDownloadObserver {
    fn on_progress(&self, progress: MithrilDownloadProgress) {
        let progress = match progress {
            selected @ MithrilDownloadProgress::SnapshotSelected { .. } => selected,
            MithrilDownloadProgress::Downloaded { downloaded_bytes, completed_files, total_files, total_bytes } => {
                let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                state.downloaded_bytes = state.downloaded_bytes.max(downloaded_bytes);
                state.completed_files = state.completed_files.max(completed_files);
                state.total_files = state.total_files.max(total_files);
                state.total_bytes = match (state.total_bytes, total_bytes) {
                    (Some(previous), Some(current)) => Some(previous.max(current)),
                    (known @ Some(_), None) => known,
                    (None, current) => current,
                };
                MithrilDownloadProgress::Downloaded {
                    downloaded_bytes: state.downloaded_bytes,
                    completed_files: state.completed_files,
                    total_files: state.total_files,
                    total_bytes: state.total_bytes,
                }
            }
        };
        self.inner.on_progress(progress);
    }
}

struct AggregatorDetails {
    endpoint: &'static str,
    verification_key: &'static str,
}

struct MithrilFeedbackReceiver {
    with_progress: ProgressFactory,
    cardano_database_pb: Mutex<Option<Box<dyn ProgressBar + Send + Sync>>>,
    certificate_validation_pb: Mutex<Option<Box<dyn ProgressBar + Send + Sync>>>,
}

#[derive(Default)]
struct CanonicalDownloadState {
    cached_bytes: u64,
    cached_files: u64,
    requested_files: u64,
    files: BTreeMap<String, (u64, u64, bool)>,
}

struct CanonicalFeedbackReceiver {
    observer: Arc<dyn MithrilDownloadObserver>,
    state: Mutex<CanonicalDownloadState>,
}

impl CanonicalFeedbackReceiver {
    fn new(observer: Arc<dyn MithrilDownloadObserver>, cached_bytes: u64, cached_files: u64) -> Self {
        Self {
            observer,
            state: Mutex::new(CanonicalDownloadState {
                cached_bytes,
                cached_files,
                ..CanonicalDownloadState::default()
            }),
        }
    }

    fn report(&self, state: &CanonicalDownloadState) {
        let downloaded_bytes = state
            .files
            .values()
            .fold(state.cached_bytes, |total, (_, downloaded, _)| total.saturating_add(*downloaded));
        let completed_files =
            state.cached_files + state.files.values().filter(|(_, _, completed)| *completed).count() as u64;
        let all_sizes_known = state.files.len() as u64 == state.requested_files;
        let total_bytes = all_sizes_known
            .then(|| state.files.values().fold(state.cached_bytes, |total, (size, _, _)| total.saturating_add(*size)));
        self.observer.on_progress(MithrilDownloadProgress::Downloaded {
            downloaded_bytes,
            completed_files,
            total_files: state.cached_files.saturating_add(state.requested_files),
            total_bytes,
        });
    }
}

#[async_trait]
#[allow(clippy::wildcard_enum_match_arm)]
impl FeedbackReceiver for CanonicalFeedbackReceiver {
    async fn handle_event(&self, event: MithrilEvent) {
        let MithrilEvent::CardanoDatabase(event) = event else {
            return;
        };
        let mut state = self.state.lock().await;
        match event {
            MithrilEventCardanoDatabase::Started { total_immutable_files, include_ancillary, .. } => {
                state.requested_files = total_immutable_files + u64::from(include_ancillary);
            }
            MithrilEventCardanoDatabase::ImmutableDownloadStarted { immutable_file_number, size, .. } => {
                state.files.insert(immutable_file_number.to_string(), (size, 0, false));
            }
            MithrilEventCardanoDatabase::ImmutableDownloadProgress {
                immutable_file_number,
                downloaded_bytes,
                size,
                ..
            } => {
                state.files.insert(immutable_file_number.to_string(), (size, downloaded_bytes, false));
            }
            MithrilEventCardanoDatabase::ImmutableDownloadCompleted { immutable_file_number, .. } => {
                if let Some((size, downloaded, completed)) = state.files.get_mut(&immutable_file_number.to_string()) {
                    *downloaded = *size;
                    *completed = true;
                }
            }
            MithrilEventCardanoDatabase::AncillaryDownloadStarted { size, .. } => {
                state.files.insert("ancillary".to_string(), (size, 0, false));
            }
            MithrilEventCardanoDatabase::AncillaryDownloadProgress { downloaded_bytes, size, .. } => {
                state.files.insert("ancillary".to_string(), (size, downloaded_bytes, false));
            }
            MithrilEventCardanoDatabase::AncillaryDownloadCompleted { .. } => {
                if let Some((size, downloaded, completed)) = state.files.get_mut("ancillary") {
                    *downloaded = *size;
                    *completed = true;
                }
            }
            _ => return,
        }
        self.report(&state);
    }
}

impl MithrilFeedbackReceiver {
    fn new(with_progress: ProgressFactory) -> Self {
        Self { with_progress, cardano_database_pb: Mutex::new(None), certificate_validation_pb: Mutex::new(None) }
    }
}

#[async_trait]
#[allow(clippy::wildcard_enum_match_arm)]
impl FeedbackReceiver for MithrilFeedbackReceiver {
    async fn handle_event(&self, event: MithrilEvent) {
        match event {
            MithrilEvent::CardanoDatabase(cardano_database_event) => match cardano_database_event {
                MithrilEventCardanoDatabase::Started { download_id: _, total_immutable_files, include_ancillary } => {
                    let size = match include_ancillary {
                        true => 1 + total_immutable_files,
                        false => total_immutable_files,
                    };
                    let pb = (self.with_progress)(
                        size as usize,
                        "{spinner:.green} Downloading Mithril files {bytes_per_sec:>10} {bar:40.green} [{pos}/{len}] ({eta} remaining)",
                    );
                    *self.cardano_database_pb.lock().await = Some(pb);
                }
                MithrilEventCardanoDatabase::Completed { .. } => {
                    if let Some(pb) = self.cardano_database_pb.lock().await.take() {
                        pb.clear();
                    }
                }
                MithrilEventCardanoDatabase::ImmutableDownloadCompleted { .. }
                | MithrilEventCardanoDatabase::AncillaryDownloadCompleted { .. } => {
                    if let Some(pb) = self.cardano_database_pb.lock().await.as_ref() {
                        pb.increment();
                    }
                }
                _ => {}
            },
            MithrilEvent::CertificateChainValidationStarted { .. } => {
                let pb = (self.with_progress)(
                    0,
                    "{spinner:.green} {elapsed_precise} validating Mithril certificate chain ({pos} certificates)",
                );
                *self.certificate_validation_pb.lock().await = Some(pb);
            }
            MithrilEvent::CertificateValidated { .. } | MithrilEvent::CertificateFetchedFromCache { .. } => {
                if let Some(pb) = self.certificate_validation_pb.lock().await.as_ref() {
                    pb.increment();
                }
            }
            MithrilEvent::CertificateChainValidated { .. } => {
                if let Some(pb) = self.certificate_validation_pb.lock().await.take() {
                    pb.clear();
                }
            }
            _ => {}
        }
    }
}

fn aggregator_details(network: NetworkName) -> anyhow::Result<AggregatorDetails> {
    match network {
        NetworkName::Mainnet => Ok(AggregatorDetails {
            endpoint: "https://aggregator.release-mainnet.api.mithril.network/aggregator",
            verification_key: "5b3139312c36362c3134302c3138352c3133382c31312c3233372c3230372c3235302c3134342c32372c322c3138382c33302c31322c38312c3135352c3230342c31302c3137392c37352c32332c3133382c3139362c3231372c352c31342c32302c35372c37392c33392c3137365d",
        }),
        NetworkName::Preprod => Ok(AggregatorDetails {
            endpoint: "https://aggregator.release-preprod.api.mithril.network/aggregator",
            verification_key: "5b3132372c37332c3132342c3136312c362c3133372c3133312c3231332c3230372c3131372c3139382c38352c3137362c3139392c3136322c3234312c36382c3132332c3131392c3134352c31332c3233322c3234332c34392c3232392c322c3234392c3230352c3230352c33392c3233352c34345d",
        }),
        NetworkName::Preview => Ok(AggregatorDetails {
            endpoint: "https://aggregator.pre-release-preview.api.mithril.network/aggregator",
            verification_key: "5b3132372c37332c3132342c3136312c362c3133372c3133312c3231332c3230372c3131372c3139382c38352c3137362c3139392c3136322c3234312c36382c3132332c3131392c3134352c31332c3233322c3234332c34392c3232392c322c3234392c3230352c3230352c33392c3233352c34345d",
        }),
        NetworkName::Testnet(_) => Err(anyhow!("Mithril is only supported on mainnet, preprod and preview")),
    }
}

pub async fn download_from_mithril(
    network: NetworkName,
    target_dir: PathBuf,
    from_chunk: u64,
    with_progress: Arc<dyn Fn(usize, &str) -> Box<dyn ProgressBar + Send + Sync> + Send + Sync>,
) -> Result<(), MithrilDownloadError> {
    download_from_mithril_with_resume_chunk(network, target_dir, from_chunk, None, with_progress, None, (0, 0))
        .await
        .map(|_| ())
}

async fn download_from_mithril_with_resume_chunk(
    network: NetworkName,
    target_dir: PathBuf,
    from_chunk: u64,
    resume_chunk: Option<u64>,
    with_progress: Arc<dyn Fn(usize, &str) -> Box<dyn ProgressBar + Send + Sync> + Send + Sync>,
    observer: Option<Arc<dyn MithrilDownloadObserver>>,
    cached: (u64, u64),
) -> Result<String, MithrilDownloadError> {
    let AggregatorDetails { endpoint, verification_key } = aggregator_details(network)?;
    let mut client_builder = ClientBuilder::new(mithril_client::AggregatorDiscoveryType::Url(endpoint.to_string()))
        .set_genesis_verification_key(GenesisVerificationKey::JsonHex(verification_key.into()))
        .with_origin_tag(Some("AMARU".to_string()));
    if let Some(observer) = observer.clone() {
        client_builder = client_builder
            .add_feedback_receiver(Arc::new(CanonicalFeedbackReceiver::new(observer, cached.0, cached.1)));
    } else {
        client_builder =
            client_builder.add_feedback_receiver(Arc::new(MithrilFeedbackReceiver::new(with_progress.clone())));
    }
    let client = client_builder.build()?;
    let database_client = client.cardano_database_v2();
    let snapshots = database_client.list().await.map_err(|source| MithrilDownloadError::Unavailable { source })?;
    let snapshot_list_item =
        snapshots.into_iter().max_by(|left, right| left.beacon.cmp(&right.beacon)).ok_or_else(|| {
            MithrilDownloadError::Unavailable { source: anyhow::anyhow!("no Mithril cardano-db snapshot found") }
        })?;

    info!(mithril::snapshot::FETCH, hash = snapshot_list_item.hash, from_chunk);

    let fetch_progress = with_progress(0, "{spinner:.green} {elapsed_precise} fetching Mithril snapshot metadata");
    let snapshot = database_client.get(&snapshot_list_item.hash).await;
    fetch_progress.clear();
    let snapshot = snapshot.map_err(|source| MithrilDownloadError::Unavailable { source })?.ok_or_else(|| {
        MithrilDownloadError::Unavailable {
            source: anyhow::anyhow!("Mithril snapshot not found: {}", snapshot_list_item.hash),
        }
    })?;
    let through_chunk = snapshot.beacon.immutable_file_number;
    validate_snapshot_range(from_chunk, resume_chunk, through_chunk)?;
    if let Some(observer) = observer.as_ref() {
        observer.on_progress(MithrilDownloadProgress::SnapshotSelected {
            hash: snapshot_list_item.hash.clone(),
            through_chunk,
        });
    }
    let certificate = client
        .certificate()
        .verify_chain(&snapshot.certificate_hash)
        .await
        .map_err(|source| MithrilDownloadError::Validation { source })?;

    let immutable_file_range = ImmutableFileRange::From(from_chunk);
    let download_unpack_options =
        DownloadUnpackOptions { allow_override: true, include_ancillary: false, ..DownloadUnpackOptions::default() };
    info!(mithril::snapshot::DOWNLOAD, target_dir = target_dir.display().to_string(), from_chunk, through_chunk);
    database_client
        .download_unpack(&snapshot, &immutable_file_range, &target_dir, download_unpack_options)
        .await
        .map_err(|source| MithrilDownloadError::Unavailable { source })?;

    info!(mithril::snapshot::VERIFY_DIGESTS, target_dir = target_dir.display().to_string());
    let verified_digests = client
        .cardano_database_v2()
        .download_and_verify_digests(&certificate, &snapshot)
        .await
        .map_err(|source| MithrilDownloadError::Validation { source })?;
    let immutable_file_count = immutable_file_range.length(through_chunk) * 3;
    info!(mithril::snapshot::VERIFY_DATABASE, target_dir = target_dir.display().to_string());
    let verification_template = format!(
        "{{spinner:.green}} {{elapsed_precise}} verifying immutable chunks {from_chunk}..={through_chunk} ({immutable_file_count} files)"
    );
    let verification_progress = with_progress(0, &verification_template);
    let merkle_proof = client
        .cardano_database_v2()
        .verify_cardano_database(&certificate, &snapshot, &immutable_file_range, false, &target_dir, &verified_digests)
        .await;
    verification_progress.clear();
    let merkle_proof = merkle_proof.map_err(|source| MithrilDownloadError::Validation { source: source.into() })?;
    let message = MessageBuilder::new()
        .compute_cardano_database_message(&certificate, &merkle_proof)
        .await
        .map_err(|source| MithrilDownloadError::Validation { source })?;

    if !certificate.match_message(&message) {
        return Err(MithrilDownloadError::Validation {
            source: anyhow::anyhow!("Mithril certificate verification failed"),
        });
    }

    info!(mithril::snapshot::READY, target_dir = target_dir.display().to_string());

    Ok(snapshot_list_item.hash)
}

fn validate_snapshot_range(
    from_chunk: u64,
    resume_chunk: Option<u64>,
    through_chunk: u64,
) -> Result<(), MithrilDownloadError> {
    if let Some(resume_chunk) = resume_chunk
        && resume_chunk > through_chunk
    {
        return Err(MithrilDownloadError::Inapplicable { through_chunk, required_chunk: resume_chunk });
    }
    if from_chunk > through_chunk {
        return Err(MithrilDownloadError::Inapplicable { through_chunk, required_chunk: from_chunk });
    }
    Ok(())
}

/// Downloads a Mithril snapshot for resuming from `resume_point`.
///
/// The selected range is bounded by both the validated cache tail and the resume point. If local
/// immutable validation fails, only the disposable `immutable` directory is rebuilt, at most once.
pub async fn download_from_mithril_for_resume_point(
    network: NetworkName,
    target_dir: PathBuf,
    resume_point: Point,
    with_progress: Arc<dyn Fn(usize, &str) -> Box<dyn ProgressBar + Send + Sync> + Send + Sync>,
) -> Result<PathBuf, MithrilDownloadError> {
    download_from_mithril_for_resume_point_inner(network, target_dir, resume_point, with_progress, None)
        .await
        .map(|report| report.immutable_dir)
}

/// Downloads a verified Mithril database while reporting renderer-independent progress.
pub async fn download_from_mithril_for_resume_point_with_observer(
    network: NetworkName,
    target_dir: PathBuf,
    resume_point: Point,
    observer: Arc<dyn MithrilDownloadObserver>,
) -> Result<MithrilDownloadReport, MithrilDownloadError> {
    download_from_mithril_for_resume_point_inner(
        network,
        target_dir,
        resume_point,
        Arc::new(|_, _| Box::new(NoProgressBar {})),
        Some(observer),
    )
    .await
}

async fn download_from_mithril_for_resume_point_inner(
    network: NetworkName,
    target_dir: PathBuf,
    resume_point: Point,
    with_progress: Arc<dyn Fn(usize, &str) -> Box<dyn ProgressBar + Send + Sync> + Send + Sync>,
    observer: Option<Arc<dyn MithrilDownloadObserver>>,
) -> Result<MithrilDownloadReport, MithrilDownloadError> {
    let immutable_dir = target_dir.join("immutable");
    let resume_chunk = chunk_for_slot(network, resume_point.slot_or_default().into())?;
    let observer =
        observer.map(|observer| Arc::new(MonotonicDownloadObserver::new(observer)) as Arc<dyn MithrilDownloadObserver>);
    let mut rebuilt = false;

    loop {
        let from_chunk = match validated_download_boundary(&immutable_dir, network, resume_point) {
            Ok(from_chunk) => from_chunk,
            Err(error) if !rebuilt => {
                rebuild_immutable_cache(&immutable_dir, &error)?;
                rebuilt = true;
                continue;
            }
            Err(error) => {
                return Err(MithrilDownloadError::InvalidCache {
                    source: error.context("immutable cache validation failed after rebuild"),
                });
            }
        };

        let snapshot_hash = match download_from_mithril_with_resume_chunk(
            network,
            target_dir.clone(),
            from_chunk,
            Some(resume_chunk),
            with_progress.clone(),
            observer.clone(),
            cached_download_state(&immutable_dir)?,
        )
        .await
        {
            Ok(snapshot_hash) => snapshot_hash,
            Err(source @ MithrilDownloadError::Inapplicable { .. }) => {
                return reuse_immutable_cache(immutable_dir, network, resume_point, observer.as_deref(), source);
            }
            Err(source) => return Err(source),
        };

        match validate_immutable_resume_point(&immutable_dir, network, resume_point) {
            Ok(()) => return Ok(MithrilDownloadReport { immutable_dir, snapshot_hash: Some(snapshot_hash) }),
            Err(error) if !rebuilt => {
                rebuild_immutable_cache(&immutable_dir, &error)?;
                rebuilt = true;
            }
            Err(error) => {
                return Err(MithrilDownloadError::InvalidCache {
                    source: error.context("immutable cache validation failed after rebuild"),
                });
            }
        }
    }
}

fn reuse_immutable_cache(
    immutable_dir: PathBuf,
    network: NetworkName,
    resume_point: Point,
    observer: Option<&dyn MithrilDownloadObserver>,
    inapplicable: MithrilDownloadError,
) -> Result<MithrilDownloadReport, MithrilDownloadError> {
    validated_download_boundary(&immutable_dir, network, resume_point).map_err(|source| {
        MithrilDownloadError::InvalidCache { source: source.context("immutable cache validation failed") }
    })?;
    validate_immutable_resume_point(&immutable_dir, network, resume_point).map_err(|_| inapplicable)?;
    let (downloaded_bytes, completed_files) = cached_download_state(&immutable_dir)?;
    if let Some(observer) = observer {
        observer.on_progress(MithrilDownloadProgress::Downloaded {
            downloaded_bytes,
            completed_files,
            total_files: completed_files,
            total_bytes: Some(downloaded_bytes),
        });
    }
    Ok(MithrilDownloadReport { immutable_dir, snapshot_hash: None })
}

fn cached_download_state(immutable_dir: &Path) -> anyhow::Result<(u64, u64)> {
    let entries = match fs::read_dir(immutable_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((0, 0)),
        Err(error) => return Err(error.into()),
    };
    let mut bytes = 0_u64;
    let mut chunks = 0_u64;
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        bytes = bytes.saturating_add(entry.metadata()?.len());
        if entry.path().extension().and_then(|extension| extension.to_str()) == Some("chunk") {
            chunks = chunks.saturating_add(1);
        }
    }
    Ok((bytes, chunks))
}

fn rebuild_immutable_cache(immutable_dir: &Path, reason: &anyhow::Error) -> anyhow::Result<()> {
    warn!(
        mithril::snapshot::REBUILD_CACHE,
        immutable_dir = immutable_dir.display().to_string(),
        reason = reason.to_string()
    );
    match fs::remove_dir_all(immutable_dir) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use amaru_kernel::Point;

    use super::*;
    use crate::immutable::tests::immutable_store;

    #[derive(Default)]
    struct RecordingObserver(Mutex<Vec<MithrilDownloadProgress>>);

    impl MithrilDownloadObserver for RecordingObserver {
        fn on_progress(&self, progress: MithrilDownloadProgress) {
            self.0.lock().unwrap().push(progress);
        }
    }

    fn inapplicable() -> MithrilDownloadError {
        MithrilDownloadError::Inapplicable { through_chunk: 1, required_chunk: 2 }
    }

    #[test]
    fn inapplicable_snapshot_reuses_a_verified_cache() {
        let (dir, blocks) = immutable_store();
        let observer = RecordingObserver::default();
        let (downloaded_bytes, completed_files) = cached_download_state(dir.path()).unwrap();

        let report = reuse_immutable_cache(
            dir.path().to_path_buf(),
            NetworkName::Preprod,
            blocks[1].0,
            Some(&observer),
            inapplicable(),
        )
        .unwrap();

        assert_eq!(report.immutable_dir, dir.path());
        assert_eq!(report.snapshot_hash, None);
        assert_eq!(
            observer.0.lock().unwrap().as_slice(),
            [MithrilDownloadProgress::Downloaded {
                downloaded_bytes,
                completed_files,
                total_files: completed_files,
                total_bytes: Some(downloaded_bytes),
            }]
        );
    }

    #[test]
    fn inapplicable_snapshot_rejects_a_cache_with_the_wrong_hash() {
        let (dir, blocks) = immutable_store();
        let resume_point = Point::Specific(blocks[1].0.slot_or_default(), [0; 32].into(), blocks[1].0.block_height());

        assert!(matches!(
            reuse_immutable_cache(dir.path().to_path_buf(), NetworkName::Preprod, resume_point, None, inapplicable()),
            Err(MithrilDownloadError::Inapplicable { .. })
        ));
    }

    #[test]
    fn inapplicable_snapshot_rejects_a_discontinuous_cache() {
        let (dir, blocks) = immutable_store();
        fs::remove_file(dir.path().join("00001.primary")).unwrap();

        assert!(matches!(
            reuse_immutable_cache(dir.path().to_path_buf(), NetworkName::Preprod, blocks[1].0, None, inapplicable()),
            Err(MithrilDownloadError::InvalidCache { .. })
        ));
    }

    #[test]
    fn progress_remains_monotonic_across_a_cache_retry() {
        let recording = Arc::new(RecordingObserver::default());
        let observer = MonotonicDownloadObserver::new(recording.clone());

        observer.on_progress(MithrilDownloadProgress::Downloaded {
            downloaded_bytes: 100,
            completed_files: 2,
            total_files: 4,
            total_bytes: Some(200),
        });
        observer.on_progress(MithrilDownloadProgress::Downloaded {
            downloaded_bytes: 25,
            completed_files: 0,
            total_files: 4,
            total_bytes: None,
        });

        assert_eq!(
            recording.0.lock().unwrap().last(),
            Some(&MithrilDownloadProgress::Downloaded {
                downloaded_bytes: 100,
                completed_files: 2,
                total_files: 4,
                total_bytes: Some(200),
            })
        );
    }

    #[test]
    fn unknown_download_sizes_remain_optional() {
        let recording = Arc::new(RecordingObserver::default());
        let receiver = CanonicalFeedbackReceiver::new(recording.clone(), 40, 1);
        receiver.report(&CanonicalDownloadState {
            cached_bytes: 40,
            cached_files: 1,
            requested_files: 2,
            ..Default::default()
        });

        assert_eq!(
            recording.0.lock().unwrap().last(),
            Some(&MithrilDownloadProgress::Downloaded {
                downloaded_bytes: 40,
                completed_files: 1,
                total_files: 3,
                total_bytes: None,
            })
        );
    }
}
