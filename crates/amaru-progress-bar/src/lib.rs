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
/// the platform.
///
/// A progress bar is active from creation until either [`ProgressBar::clear`] or
/// [`ProgressBar::finish`] consumes it. It cannot be restarted after either terminal operation.
pub trait ProgressBar: Send + Sync {
    /// Advance the reported progress by `size` caller-defined units.
    ///
    /// The unit must match the `length` passed to [`ProgressBarFactory::create_for`]: for example,
    /// bytes for a download, entries for an import, or milestones for a multi-step operation.
    /// A declared length may be an estimate, so implementations must tolerate cumulative progress
    /// exceeding it.
    fn tick(&self, size: usize);

    /// Advance the reported progress by one unit.
    ///
    /// This is equivalent to `tick(1)` and is intended for a completed item or milestone.
    fn increment(&self) {
        self.tick(1);
    }

    /// Refresh the visual indicator without advancing the reported progress.
    ///
    /// Implementations without an animated or interactive representation may ignore this.
    fn refresh(&self) {
        self.tick(0);
    }

    /// Cancel the tracked operation and remove its visual indicator.
    ///
    /// Cancellation is terminal, so this consumes the progress-bar handle. No subsequent ticks or
    /// terminal operation are possible through that handle.
    fn clear(self: Box<Self>);

    /// Mark the tracked operation as successfully completed and remove its visual indicator.
    ///
    /// Renderers that distinguish completion from cancellation can override this method. The
    /// default treats completion as a clear. Completion is terminal and may occur before the
    /// declared length is reached, so this consumes the progress-bar handle.
    fn finish(self: Box<Self>) {
        self.clear();
    }
}

/// Creates progress indicators while allowing callers to attach a stable phase name.
///
/// The blanket implementation preserves the existing `(length, template)` closure API by ignoring
/// the phase name. Renderers that need semantic phase names, such as structured non-terminal
/// logging, can implement this trait directly.
pub trait ProgressBarFactory {
    /// Create a progress bar for a named phase.
    ///
    /// `length` is the expected total in the same caller-defined unit later passed to
    /// [`ProgressBar::tick`]. A length of zero means that the total is unknown.
    fn create_for(&self, phase: &'static str, length: usize, template: &str) -> Box<dyn ProgressBar>;
}

impl<F> ProgressBarFactory for F
where
    F: Fn(usize, &str) -> Box<dyn ProgressBar>,
{
    fn create_for(&self, _phase: &'static str, length: usize, template: &str) -> Box<dyn ProgressBar> {
        self(length, template)
    }
}

mod no_progress_bar;
pub use no_progress_bar::*;

#[cfg(feature = "terminal")]
mod terminal_progress_bar;
#[cfg(feature = "terminal")]
pub use terminal_progress_bar::*;
