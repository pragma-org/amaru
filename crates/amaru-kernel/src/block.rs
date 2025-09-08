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

use crate::peer::Peer;
use crate::{Point, RawBlock};
use pallas_primitives::babbage::Header;
use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use tracing::Span;

#[derive(Clone, Serialize, Deserialize)]
pub enum ValidateBlockEvent {
    Validated {
        peer: Peer,
        header: Header,
        #[serde(skip, default = "default_block")]
        block: Arc<RawBlock>,
        #[serde(skip, default = "Span::none")]
        span: Span,
    },
    Rollback {
        peer: Peer,
        rollback_point: Point,
        #[serde(skip, default = "Span::none")]
        span: Span,
    },
}

fn default_block() -> Arc<RawBlock> {
    Arc::new(Vec::new())
}

impl Debug for ValidateBlockEvent {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidateBlockEvent::Validated {
                peer,
                header,
                block,
                ..
            } => f
                .debug_struct("Validated")
                .field("peer", peer)
                .field("header", header)
                .field("block", block)
                .finish(),
            ValidateBlockEvent::Rollback {
                peer,
                rollback_point,
                ..
            } => f
                .debug_struct("Rollback")
                .field("peer", peer)
                .field("rollback_point", rollback_point)
                .finish(),
        }
    }
}

impl PartialEq for ValidateBlockEvent {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                ValidateBlockEvent::Validated {
                    peer: p1,
                    header: h1,
                    block: b1,
                    ..
                },
                ValidateBlockEvent::Validated {
                    peer: p2,
                    header: h2,
                    block: b2,
                    ..
                },
            ) => p1 == p2 && h1 == h2 && Arc::ptr_eq(b1, b2),
            (
                ValidateBlockEvent::Rollback {
                    peer: p1,
                    rollback_point: rp1,
                    ..
                },
                ValidateBlockEvent::Rollback {
                    peer: p2,
                    rollback_point: rp2,
                    ..
                },
            ) => p1 == p2 && rp1 == rp2,
            _ => false,
        }
    }
}

#[derive(Debug, Clone)]
pub enum BlockValidationResult {
    BlockValidated {
        point: Point,
        block: RawBlock,
        span: Span,
        block_height: u64,
    },
    BlockValidationFailed {
        point: Point,
        span: Span,
    },
    RolledBackTo {
        rollback_point: Point,
        span: Span,
    },
}
