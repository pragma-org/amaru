// Copyright 2024 PRAGMA
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

use amaru_kernel::Point;
use tracing::Span;

pub type RawBlock = Vec<u8>;

#[derive(Clone)]
pub enum ValidateBlockEvent {
    Validated(Point, RawBlock, Span),
    Rollback(Point),
}

#[derive(Clone)]
pub enum BlockValidationResult {
    BlockValidated(Point, Span),
    BlockForwardStorageFailed(Point, Span),
    InvalidRollbackPoint(Point),
    RolledBackTo(Point),
}

/// Iterators
///
/// A set of additional primitives around iterators. Not Amaru-specific so-to-speak.
pub mod rewards;
pub mod state;
pub mod store;
