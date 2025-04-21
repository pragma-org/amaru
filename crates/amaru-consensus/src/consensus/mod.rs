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

use crate::peer::Peer;

pub mod chain_selection;
pub mod header_validation;
pub mod receive_header;
pub mod store;

#[derive(Clone, Debug)]
pub enum PullEvent {
    RollForward(Peer, Point, Vec<u8>, Span),
    Rollback(Peer, Point),
}

#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum ValidateHeaderEvent {
    Validated(Peer, Point, Span),
    Rollback(Point, Span),
}
