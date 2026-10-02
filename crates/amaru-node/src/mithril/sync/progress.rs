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
    io::{self, IsTerminal},
    sync::Mutex,
    time::{Duration, Instant},
};

use amaru_kernel::Point;
use amaru_observability::info;
use amaru_progress_bar::{ProgressBar, TerminalProgressBar};

const STRUCTURED_PROGRESS_INTERVAL: Duration = Duration::from_secs(5);

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
    planned_blocks: Option<u64>,
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
                    MithrilStage::Ingesting if self.planned_blocks.is_some_and(|total| total > 0) => {
                        Some("{spinner:.green} Ingesting {pos}/{len} blocks {wide_bar:.green} {per_sec} ETA {eta}")
                    }
                    MithrilStage::Ingesting => Some("{spinner:.green} Ingesting {pos} blocks ({per_sec})"),
                    MithrilStage::ResolvingResumePoint
                    | MithrilStage::FetchingSnapshot
                    | MithrilStage::Downloading
                    | MithrilStage::DatabaseVerified
                    | MithrilStage::RecoveringStores => None,
                };
                let length = if stage == MithrilStage::Ingesting { self.planned_blocks.unwrap_or(0) } else { 0 };
                self.verification = template.map(|template| TerminalProgressBar::new(length, template).boxed());
            }
            MithrilProgress::IngestPlanned { total_blocks } => self.planned_blocks = Some(total_blocks),
            MithrilProgress::CertificateValidated => {
                if let Some(verification) = &self.verification {
                    verification.increment();
                }
            }
            MithrilProgress::Completed { .. } => {
                if let Some(download) = self.download.take() {
                    download.finish();
                }
                if let Some(verification) = self.verification.take() {
                    verification.finish();
                }
            }
            MithrilProgress::BlocksIngested { blocks, .. } => {
                if let Some(verification) = &self.verification {
                    verification
                        .tick(usize::try_from(blocks.saturating_sub(self.ingested_blocks)).unwrap_or(usize::MAX));
                }
                self.ingested_blocks = blocks;
            }
            MithrilProgress::SnapshotSelected { .. } => {}
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
