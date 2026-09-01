// Copyright 2025 PRAGMA
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

use std::time::Duration;

use indicatif::ProgressStyle;

use super::ProgressBar;

/// A simple progress bar in ther terminal.
pub struct TerminalProgressBar {
    inner: indicatif::ProgressBar,
}

impl TerminalProgressBar {
    #[expect(clippy::unwrap_used)]
    pub fn new(size: impl Into<u64>, template: impl AsRef<str>) -> Self {
        let size = size.into();
        let inner = indicatif::ProgressBar::new(size)
            .with_style(ProgressStyle::with_template(template.as_ref()).unwrap().progress_chars("█▉▊▋▌▍▎▏-"));
        if size == 0 {
            inner.enable_steady_tick(Duration::from_millis(100));
        }
        Self { inner }
    }

    pub fn boxed(self) -> Box<dyn ProgressBar> {
        Box::new(self)
    }

    /// Finish a terminal progress bar that is shared with progress-reporting callbacks.
    pub fn finish_and_clear(&self) {
        self.inner.finish_and_clear();
    }
}

impl ProgressBar for TerminalProgressBar {
    fn tick(&self, size: usize) {
        self.inner.inc(size as u64);
    }

    fn clear(self: Box<Self>) {
        self.finish_and_clear();
    }
}
