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

/// A thin abstraction to notify progress on a task, without committing to any particular tool.
///
/// The main use case is to allow decoupling certain long-running functions from their UI elements,
/// so that they can be re-used in tests and be part of crates that must compile irrespective of
/// the platform
pub trait ProgressBar: Send + Sync {
    fn tick(&self, size: usize);
    fn clear(&self);

    /// Mark the tracked operation as successfully completed and remove its visual indicator.
    ///
    /// Renderers that distinguish completion from cancellation can override this method. The
    /// default keeps existing progress bars source-compatible by treating completion as a clear.
    fn finish(&self) {
        self.clear();
    }
}

/// Creates progress indicators while allowing callers to attach a stable phase name.
///
/// The blanket implementation preserves the existing `(length, template)` closure API. Renderers
/// that need semantic phase names, such as structured non-terminal logging, can implement this
/// trait directly and override [`ProgressBarFactory::create_for`].
pub trait ProgressBarFactory {
    fn create(&self, length: usize, template: &str) -> Box<dyn ProgressBar>;

    fn create_for(&self, _phase: &'static str, length: usize, template: &str) -> Box<dyn ProgressBar> {
        self.create(length, template)
    }
}

impl<F> ProgressBarFactory for F
where
    F: Fn(usize, &str) -> Box<dyn ProgressBar>,
{
    fn create(&self, length: usize, template: &str) -> Box<dyn ProgressBar> {
        self(length, template)
    }
}

mod no_progress_bar;
pub use no_progress_bar::*;

#[cfg(feature = "terminal")]
mod terminal_progress_bar;
#[cfg(feature = "terminal")]
pub use terminal_progress_bar::*;
