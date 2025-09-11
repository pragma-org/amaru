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

/// A dummy implementation of 'ProgressBar' which doesn't do anything.
pub struct NoProgressBar {}

pub fn no_progress_bar(_length: usize, _template: &str) -> Box<dyn ProgressBar> {
    Box::new(NoProgressBar {})
}

impl ProgressBar for NoProgressBar {
    fn tick(&self, _size: usize) {}
    fn clear(&self) {}
}
