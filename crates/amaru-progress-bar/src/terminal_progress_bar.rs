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
    io::{self, Write},
    sync::{
        LazyLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use indicatif::{MultiProgress, ProgressStyle};

use super::ProgressBar;

/// A simple progress bar in the terminal.
pub struct TerminalProgressBar {
    inner: indicatif::ProgressBar,
}

static PROGRESS: LazyLock<MultiProgress> = LazyLock::new(MultiProgress::new);
static ACTIVE_BARS: AtomicUsize = AtomicUsize::new(0);

/// A tracing writer that prints complete console lines above active progress bars.
pub struct ProgressLogWriter(Vec<u8>);

/// Create a writer for a single console log event.
pub fn progress_log_writer() -> ProgressLogWriter {
    ProgressLogWriter(Vec::new())
}

impl Write for ProgressLogWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.0.is_empty() {
            write_log(&PROGRESS, ACTIVE_BARS.load(Ordering::Acquire) > 0, &self.0)?;
            self.0.clear();
        }
        Ok(())
    }
}

impl Drop for ProgressLogWriter {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

fn write_log(progress: &MultiProgress, has_active_bar: bool, bytes: &[u8]) -> io::Result<()> {
    if has_active_bar && !progress.is_hidden() {
        let line = String::from_utf8_lossy(bytes);
        progress.println(line.strip_suffix('\n').unwrap_or(&line))
    } else {
        io::stderr().write_all(bytes)
    }
}

impl TerminalProgressBar {
    #[expect(clippy::unwrap_used)]
    pub fn new(size: impl Into<u64>, template: impl AsRef<str>) -> Self {
        let size = size.into();
        let style = ProgressStyle::with_template(template.as_ref()).unwrap().progress_chars("█▉▊▋▌▍▎▏-");
        ACTIVE_BARS.fetch_add(1, Ordering::Release);
        let inner = PROGRESS.add(indicatif::ProgressBar::new(size).with_style(style));
        if size == 0 {
            inner.enable_steady_tick(Duration::from_millis(100));
        }
        Self { inner }
    }

    pub fn boxed(self) -> Box<dyn ProgressBar> {
        Box::new(self)
    }

    /// Cancel a terminal progress bar shared with progress-reporting callbacks.
    pub fn clear_shared(&self) {
        self.inner.finish_and_clear();
    }

    /// Clear a shared terminal progress bar, then log the completed work.
    ///
    /// Stop progress-reporting callbacks before calling this method.
    pub fn finish_shared(&self, summary: impl FnOnce()) {
        self.clear_shared();
        summary();
    }
}

impl Drop for TerminalProgressBar {
    fn drop(&mut self) {
        PROGRESS.remove(&self.inner);
        ACTIVE_BARS.fetch_sub(1, Ordering::Release);
    }
}

impl ProgressBar for TerminalProgressBar {
    fn tick(&self, size: usize) {
        self.inner.inc(size as u64);
    }

    fn clear(self: Box<Self>) {
        self.clear_shared();
    }
}
