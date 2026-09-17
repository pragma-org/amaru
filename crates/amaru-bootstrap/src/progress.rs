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

use amaru_kernel::{Epoch, NetworkPoint};
use amaru_observability::info;
use amaru_progress_bar::{NoProgressBar, ProgressBar, ProgressBarFactory, TerminalProgressBar};

const STRUCTURED_DOWNLOAD_INTERVAL: Duration = Duration::from_secs(5);

/// A high-level stage in the bootstrap process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BootstrapStage {
    /// Discover the snapshots available for the selected network.
    DiscoveringSnapshots,
    /// Choose and inspect the three-snapshot bootstrap window.
    SelectingSnapshots,
    /// Download or reuse the selected snapshot archives.
    DownloadingSnapshots,
    /// Import the selected snapshots into the ledger store.
    ImportingSnapshots,
    /// Seed the chain store from the imported snapshot state.
    InitializingChainStore,
}

impl BootstrapStage {
    fn as_str(self) -> &'static str {
        match self {
            Self::DiscoveringSnapshots => "discovering_snapshots",
            Self::SelectingSnapshots => "selecting_snapshots",
            Self::DownloadingSnapshots => "downloading_snapshots",
            Self::ImportingSnapshots => "importing_snapshots",
            Self::InitializingChainStore => "initializing_chain_store",
        }
    }
}

/// Canonical progress reported by a bootstrap operation.
///
/// Download counters are absolute across all selected snapshots and include archives reused from
/// the cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BootstrapProgress {
    /// Bootstrap moved to a new high-level stage.
    StageChanged { stage: BootstrapStage },
    /// The bootstrap snapshot window was selected.
    SnapshotsSelected {
        snapshot_count: usize,
        /// Sum of compressed archive sizes when every selected size is known.
        total_bytes: Option<u64>,
    },
    /// Absolute download state across the selected snapshot window.
    DownloadProgress { downloaded_bytes: u64, completed_snapshots: usize },
    /// Bootstrap completed successfully.
    Completed { epoch: Epoch, point: NetworkPoint },
}

/// Receives canonical bootstrap progress.
///
/// Observers are passive: return values cannot influence snapshot selection, downloads, imports,
/// cancellation, or completion.
pub trait BootstrapObserver: Send + Sync {
    fn on_progress(&self, progress: BootstrapProgress);
}

/// The standard bootstrap observer used by [`crate::bootstrap`].
///
/// It renders an aggregate download bar on an interactive terminal and emits structured telemetry
/// otherwise. Renderer selection does not affect the events produced by bootstrap.
pub(crate) struct DefaultBootstrapObserver {
    renderer: Mutex<DefaultRenderer>,
}

impl DefaultBootstrapObserver {
    pub(crate) fn new() -> Self {
        let renderer = if io::stderr().is_terminal() {
            DefaultRenderer::Terminal(TerminalRenderer::default())
        } else {
            DefaultRenderer::Structured(StructuredRenderer::default())
        };
        Self { renderer: Mutex::new(renderer) }
    }
}

impl BootstrapObserver for DefaultBootstrapObserver {
    fn on_progress(&self, progress: BootstrapProgress) {
        let mut renderer = self.renderer.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        renderer.render(progress);
    }
}

enum DefaultRenderer {
    Terminal(TerminalRenderer),
    Structured(StructuredRenderer),
}

impl DefaultRenderer {
    fn render(&mut self, progress: BootstrapProgress) {
        match self {
            Self::Terminal(renderer) => renderer.render(progress),
            Self::Structured(renderer) => renderer.render(progress),
        }
    }
}

#[derive(Default)]
struct TerminalRenderer {
    total_bytes: Option<u64>,
    downloaded_bytes: u64,
    download_progress: Option<Box<dyn ProgressBar>>,
}

impl TerminalRenderer {
    fn render(&mut self, progress: BootstrapProgress) {
        match progress {
            BootstrapProgress::SnapshotsSelected { total_bytes, .. } => self.total_bytes = total_bytes,
            BootstrapProgress::StageChanged { stage: BootstrapStage::DownloadingSnapshots } => {
                self.download_progress = Some(
                    TerminalProgressBar::new(
                        self.total_bytes.unwrap_or(0),
                        "{spinner:.green} Downloading snapshots {bytes_per_sec:>10} {bar:40.green} [{bytes:>10}/{total_bytes:<10}] ({eta} remaining)",
                    )
                    .boxed(),
                );
            }
            BootstrapProgress::DownloadProgress { downloaded_bytes, .. } => {
                let delta = downloaded_bytes.saturating_sub(self.downloaded_bytes);
                self.downloaded_bytes = downloaded_bytes;
                if let Some(progress) = self.download_progress.as_ref() {
                    progress.tick(usize::try_from(delta).unwrap_or(usize::MAX));
                }
            }
            BootstrapProgress::StageChanged { .. } | BootstrapProgress::Completed { .. } => {
                if let Some(progress) = self.download_progress.take() {
                    progress.finish();
                }
            }
        }
    }
}

#[derive(Default)]
struct StructuredRenderer {
    last_download_at: Option<Instant>,
    completed_snapshots: usize,
}

impl StructuredRenderer {
    fn render(&mut self, progress: BootstrapProgress) {
        match progress {
            BootstrapProgress::StageChanged { stage } => {
                info!(bootstrap::progress::STAGE, stage = stage.as_str().to_owned());
            }
            BootstrapProgress::SnapshotsSelected { snapshot_count, total_bytes } => {
                info!(bootstrap::progress::SNAPSHOTS_SELECTED, snapshot_count, total_bytes = @total_bytes);
            }
            BootstrapProgress::DownloadProgress { downloaded_bytes, completed_snapshots } => {
                let now = Instant::now();
                let snapshot_completed = completed_snapshots > self.completed_snapshots;
                let interval_elapsed = self.last_download_at.is_none_or(|last_download_at| {
                    now.duration_since(last_download_at) >= STRUCTURED_DOWNLOAD_INTERVAL
                });
                self.completed_snapshots = completed_snapshots;
                if snapshot_completed || interval_elapsed {
                    self.last_download_at = Some(now);
                    info!(bootstrap::progress::DOWNLOAD, downloaded_bytes, completed_snapshots);
                }
            }
            BootstrapProgress::Completed { epoch, point } => {
                info!(bootstrap::progress::COMPLETE, epoch, point = point.to_string());
            }
        }
    }
}

/// Keeps detailed import progress terminal-only. Canonical bootstrap state is reported through
/// [`BootstrapObserver`], independently from this lower-level decoder progress.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BootstrapProgressFactory;

impl ProgressBarFactory for BootstrapProgressFactory {
    fn create_for(&self, _phase: &'static str, length: usize, template: &str) -> Box<dyn ProgressBar> {
        if io::stderr().is_terminal() {
            TerminalProgressBar::new(length as u64, template).boxed()
        } else {
            Box::new(NoProgressBar {})
        }
    }
}
