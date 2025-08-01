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
pub trait ProgressBar {
    fn tick(&self, size: usize);
    fn clear(&self);
}

mod no_progress_bar;
pub use no_progress_bar::*;

#[cfg(feature = "terminal")]
mod terminal_progress_bar;
#[cfg(feature = "terminal")]
pub use terminal_progress_bar::*;
