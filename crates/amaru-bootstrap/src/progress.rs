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
    time::Duration,
};

use amaru_observability::info;
use amaru_progress_bar::{ProgressBar, ProgressBarFactory, TerminalProgressBar};
use tokio::time::Instant;

const LOG_INTERVAL: Duration = Duration::from_secs(5);

/// Selects an interactive progress bar or structured progress events for bootstrap work.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BootstrapProgressFactory;

impl ProgressBarFactory for BootstrapProgressFactory {
    fn create_for(&self, phase: &'static str, length: usize, template: &str) -> Box<dyn ProgressBar> {
        if io::stderr().is_terminal() {
            TerminalProgressBar::new(length as u64, template).boxed()
        } else {
            Box::new(StructuredProgressBar::new(phase, (length > 0).then_some(length)))
        }
    }
}

struct StructuredProgressBar {
    phase: &'static str,
    total: Option<usize>,
    started_at: Instant,
    state: Mutex<ProgressState>,
}

impl StructuredProgressBar {
    fn new(phase: &'static str, total: Option<usize>) -> Self {
        let started_at = Instant::now();
        info!(bootstrap::progress::START, phase = phase.to_owned(), total = @total);
        Self { phase, total, started_at, state: Mutex::new(ProgressState::new()) }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, ProgressState> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn emit_update(&self, current: usize) {
        info!(
            bootstrap::progress::UPDATE,
            phase = self.phase.to_owned(),
            current,
            total = @self.total,
            elapsed_seconds = self.started_at.elapsed().as_secs_f64(),
        );
    }

    fn cancel(&self) {
        let current = self.state().cancel();
        if let Some(current) = current {
            info!(
                bootstrap::progress::CANCEL,
                phase = self.phase.to_owned(),
                current,
                total = @self.total,
                elapsed_seconds = self.started_at.elapsed().as_secs_f64(),
            );
        }
    }
}

impl Drop for StructuredProgressBar {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl ProgressBar for StructuredProgressBar {
    fn tick(&self, size: usize) {
        let current = self.state().advance(size, self.started_at.elapsed());
        if let Some(current) = current {
            self.emit_update(current);
        }
    }

    fn clear(self: Box<Self>) {
        self.cancel();
    }

    fn finish(self: Box<Self>) {
        let current = self.state().finish();
        if let Some(current) = current {
            info!(
                bootstrap::progress::COMPLETE,
                phase = self.phase.to_owned(),
                current,
                total = @self.total,
                elapsed_seconds = self.started_at.elapsed().as_secs_f64(),
            );
        }
    }
}

struct ProgressState {
    current: usize,
    finished: bool,
    last_emitted_at: Duration,
}

impl ProgressState {
    fn new() -> Self {
        Self { current: 0, finished: false, last_emitted_at: Duration::ZERO }
    }

    fn advance(&mut self, size: usize, elapsed: Duration) -> Option<usize> {
        if self.finished {
            return None;
        }

        self.current = self.current.saturating_add(size);
        if size == 0 || elapsed.saturating_sub(self.last_emitted_at) < LOG_INTERVAL {
            return None;
        }

        self.last_emitted_at = elapsed;
        Some(self.current)
    }

    fn cancel(&mut self) -> Option<usize> {
        if self.finished {
            return None;
        }

        self.finished = true;
        Some(self.current)
    }

    fn finish(&mut self) -> Option<usize> {
        if self.finished {
            return None;
        }

        self.finished = true;
        Some(self.current)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::sync_channel;

    use amaru_observability::{TelemetryCaptureLayer, tracing, tracing_subscriber};
    use tracing_subscriber::prelude::*;

    use super::*;

    fn capture_progress_events(run: impl FnOnce()) -> Vec<String> {
        let (tx, rx) = sync_channel(8);
        let subscriber = tracing_subscriber::registry().with(TelemetryCaptureLayer::new(tx));

        tracing::subscriber::with_default(subscriber, run);

        rx.try_iter().map(|record| record.name).collect()
    }

    #[test]
    fn tracks_updates_and_always_reports_final_position() {
        let mut state = ProgressState::new();

        assert_eq!(state.advance(10, Duration::ZERO), None);
        assert_eq!(state.advance(5, LOG_INTERVAL - Duration::from_millis(1)), None);
        assert_eq!(state.advance(7, LOG_INTERVAL), Some(22));
        assert_eq!(state.advance(3, LOG_INTERVAL + Duration::from_secs(1)), None);
        assert_eq!(state.advance(4, LOG_INTERVAL + LOG_INTERVAL), Some(29));

        assert_eq!(state.finish(), Some(29));
        assert_eq!(state.finish(), None);
        assert_eq!(state.advance(1, LOG_INTERVAL + LOG_INTERVAL + LOG_INTERVAL), None);
    }

    #[test]
    fn cancellation_does_not_report_completion() {
        let mut state = ProgressState::new();

        state.advance(10, Duration::ZERO);
        state.cancel();

        assert_eq!(state.finish(), None);
    }

    #[test]
    fn dropping_an_unfinished_progress_bar_cancels_it() {
        let events = capture_progress_events(|| {
            let progress = StructuredProgressBar::new("test", Some(10));
            progress.tick(3);
        });

        assert_eq!(events, ["progress.start", "progress.cancel"]);
    }

    #[test]
    fn terminal_calls_do_not_emit_an_additional_cancel_on_drop() {
        let cleared = capture_progress_events(|| {
            Box::new(StructuredProgressBar::new("test", Some(10))).clear();
        });
        assert_eq!(cleared, ["progress.start", "progress.cancel"]);

        let finished = capture_progress_events(|| {
            Box::new(StructuredProgressBar::new("test", Some(10))).finish();
        });
        assert_eq!(finished, ["progress.start", "progress.complete"]);
    }
}
