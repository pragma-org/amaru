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

#[cfg(any(test, feature = "test-utils"))]
#[expect(clippy::module_inception, clippy::unwrap_used)]
pub mod data_generation;

#[expect(clippy::module_inception)]
pub mod headers_tree;
pub mod headers_tree_display;
pub mod tree;

pub use headers_tree::*;
pub use headers_tree_display::*;
