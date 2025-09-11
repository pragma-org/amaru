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

use super::ProgressBar;
use indicatif::ProgressStyle;

/// A simple progress bar in ther terminal.
pub struct TerminalProgressBar {
    inner: indicatif::ProgressBar,
}

#[expect(clippy::unwrap_used)]
pub fn new_terminal_progress_bar(size: usize, template: &str) -> Box<dyn ProgressBar> {
    Box::new(TerminalProgressBar {
        inner: indicatif::ProgressBar::new(size as u64)
            .with_style(ProgressStyle::with_template(template).unwrap()),
    })
}

impl ProgressBar for TerminalProgressBar {
    fn tick(&self, size: usize) {
        self.inner.inc(size as u64);
    }

    fn clear(&self) {
        self.inner.finish_and_clear();
    }
}
