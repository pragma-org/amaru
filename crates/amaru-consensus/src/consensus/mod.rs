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

use amaru_kernel::{Header, Point};
use tracing::Span;

use crate::peer::Peer;

pub mod chain_selection;
pub mod receive_header;
pub mod select_chain;
pub mod store;
pub mod store_header;
pub mod store_block;
pub mod validate_header;

pub const EVENT_TARGET: &str = "amaru::consensus";

#[derive(Clone, Debug)]
pub enum ChainSyncEvent {
    RollForward {
        peer: Peer,
        point: Point,
        raw_header: Vec<u8>,
        span: Span,
    },
    Rollback {
        peer: Peer,
        rollback_point: Point,
        span: Span,
    },
}

#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum DecodedChainSyncEvent {
    RollForward {
        peer: Peer,
        point: Point,
        header: Header,
        span: Span,
    },
    Rollback {
        peer: Peer,
        rollback_point: Point,
        span: Span,
    },
}

#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum ValidateHeaderEvent {
    Validated {
        peer: Peer,
        point: Point,
        span: Span,
    },
    Rollback {
        peer: Peer,
        rollback_point: Point,
        span: Span,
    },
}
