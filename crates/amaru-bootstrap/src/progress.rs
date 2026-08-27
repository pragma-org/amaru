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
    sync::{Arc, Mutex, Weak},
    thread,
    time::{Duration, Instant},
};

use amaru_observability::info;
use amaru_progress_bar::{ProgressBar, ProgressBarFactory, TerminalProgressBar};

const LOG_INTERVAL: Duration = Duration::from_secs(30);

/// Selects an interactive progress bar or structured progress events for bootstrap work.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BootstrapProgressFactory;

impl ProgressBarFactory for BootstrapProgressFactory {
    fn create(&self, length: usize, template: &str) -> Box<dyn ProgressBar> {
        self.create_for("bootstrap", length, template)
    }

    fn create_for(&self, phase: &'static str, length: usize, template: &str) -> Box<dyn ProgressBar> {
        if io::stderr().is_terminal() {
            TerminalProgressBar::new(length as u64, template).boxed()
        } else {
            Box::new(StructuredProgressBar::new(phase, (length > 0).then_some(length)))
        }
    }
}

struct StructuredProgressBar {
    inner: Arc<StructuredProgress>,
}

struct StructuredProgress {
    phase: &'static str,
    total: Option<usize>,
    started_at: Instant,
    state: Mutex<ProgressState>,
}

impl StructuredProgressBar {
    fn new(phase: &'static str, total: Option<usize>) -> Self {
        let started_at = Instant::now();
        info!(bootstrap::progress::START, phase = phase.to_owned(), total = @total);
        let inner = Arc::new(StructuredProgress { phase, total, started_at, state: Mutex::new(ProgressState::new()) });
        spawn_heartbeat(Arc::downgrade(&inner));
        Self { inner }
    }
}

impl StructuredProgress {
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
}

impl ProgressBar for StructuredProgressBar {
    fn tick(&self, size: usize) {
        self.inner.state().advance(size);
    }

    fn clear(&self) {
        self.inner.state().cancel();
    }

    fn finish(&self) {
        let current = self.inner.state().finish();
        if let Some(current) = current {
            info!(
                bootstrap::progress::COMPLETE,
                phase = self.inner.phase.to_owned(),
                current,
                total = @self.inner.total,
                elapsed_seconds = self.inner.started_at.elapsed().as_secs_f64(),
            );
        }
    }
}

fn spawn_heartbeat(progress: Weak<StructuredProgress>) {
    let _heartbeat = thread::Builder::new().name("amaru-bootstrap-progress".to_owned()).spawn(move || {
        loop {
            thread::sleep(LOG_INTERVAL);
            let Some(progress) = progress.upgrade() else {
                break;
            };
            let state = progress.state();
            match state.snapshot() {
                Some(current) => progress.emit_update(current),
                None => break,
            }
        }
    });
}

struct ProgressState {
    current: usize,
    finished: bool,
}

impl ProgressState {
    fn new() -> Self {
        Self { current: 0, finished: false }
    }

    fn advance(&mut self, size: usize) {
        if self.finished {
            return;
        }

        self.current = self.current.saturating_add(size);
    }

    fn snapshot(&self) -> Option<usize> {
        (!self.finished).then_some(self.current)
    }

    fn cancel(&mut self) {
        self.finished = true;
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
    use super::*;

    #[test]
    fn tracks_updates_and_always_reports_final_position() {
        let mut state = ProgressState::new();

        state.advance(10);
        state.advance(5);
        assert_eq!(state.snapshot(), Some(15));
        state.advance(7);
        assert_eq!(state.finish(), Some(22));
        assert_eq!(state.finish(), None);
        state.advance(1);
        assert_eq!(state.snapshot(), None);
    }

    #[test]
    fn cancellation_does_not_report_completion() {
        let mut state = ProgressState::new();

        state.advance(10);
        state.cancel();

        assert_eq!(state.finish(), None);
    }
}
