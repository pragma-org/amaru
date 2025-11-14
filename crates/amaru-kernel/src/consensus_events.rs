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

use crate::{BlockHeader, IsHeader, Point, RawBlock, peer::Peer};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fmt::{Debug, Formatter};
use tracing::Span;

/// Wrapper type to factor out caught-up messages from real events.
#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Tracked<T> {
    CaughtUp {
        peer: Peer,
        #[serde(skip, default = "Span::none")]
        span: Span,
    },
    Wrapped(T),
}

impl<T: Debug> fmt::Debug for Tracked<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Tracked::Wrapped(e) => write!(f, "{:?}", e),
            Tracked::CaughtUp { peer, .. } => f
                .debug_struct("CaughtUp")
                .field("peer", &peer.name)
                .finish(),
        }
    }
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ChainSyncEvent {
    RollForward {
        peer: Peer,
        point: Point,
        raw_header: Vec<u8>,
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

impl fmt::Debug for ChainSyncEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChainSyncEvent::RollForward {
                peer,
                point,
                raw_header,
                ..
            } => f
                .debug_struct("RollForward")
                .field("peer", &peer.name)
                .field("point", &point.to_string())
                .field(
                    "raw_header",
                    &hex::encode(&raw_header[..raw_header.len().min(8)]),
                )
                .finish(),
            ChainSyncEvent::Rollback {
                peer,
                rollback_point,
                ..
            } => f
                .debug_struct("Rollback")
                .field("peer", &peer.name)
                .field("rollback_point", &rollback_point.to_string())
                .finish(),
        }
    }
}

impl ChainSyncEvent {
    pub fn set_span(&mut self, span: Span) {
        match self {
            ChainSyncEvent::RollForward { span: s, .. } => {
                *s = span;
            }
            ChainSyncEvent::Rollback { span: s, .. } => {
                *s = span;
            }
        }
    }
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum DecodedChainSyncEvent {
    RollForward {
        peer: Peer,
        header: BlockHeader,
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

impl DecodedChainSyncEvent {
    pub fn peer(&self) -> Peer {
        match self {
            DecodedChainSyncEvent::RollForward { peer, .. } => peer.clone(),
            DecodedChainSyncEvent::Rollback { peer, .. } => peer.clone(),
        }
    }

    pub fn point(&self) -> Point {
        match self {
            DecodedChainSyncEvent::RollForward { header, .. } => header.point(),
            DecodedChainSyncEvent::Rollback { rollback_point, .. } => rollback_point.clone(),
        }
    }
}

impl fmt::Debug for DecodedChainSyncEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodedChainSyncEvent::RollForward { peer, header, .. } => f
                .debug_struct("RollForward")
                .field("peer", &peer.name)
                .field("header", &header.hash().to_string())
                .finish(),
            DecodedChainSyncEvent::Rollback {
                peer,
                rollback_point,
                ..
            } => f
                .debug_struct("Rollback")
                .field("peer", &peer.name)
                .field("rollback_point", &rollback_point)
                .finish(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum ValidateHeaderEvent {
    Validated {
        peer: Peer,
        header: BlockHeader,
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

#[derive(Clone, Serialize, Deserialize)]
pub enum ValidateBlockEvent {
    Validated {
        peer: Peer,
        header: BlockHeader,
        #[serde(skip, default = "default_block")]
        block: RawBlock,
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

fn default_block() -> RawBlock {
    RawBlock::from(Vec::new().as_slice())
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
                    ..
                },
                ValidateBlockEvent::Validated {
                    peer: p2,
                    header: h2,
                    ..
                },
            ) => p1 == p2 && h1 == h2,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BlockValidationResult {
    BlockValidated {
        peer: Peer,
        header: BlockHeader,
        #[serde(skip, default = "Span::none")]
        span: Span,
    },
    BlockValidationFailed {
        peer: Peer,
        point: Point,
        #[serde(skip, default = "Span::none")]
        span: Span,
    },
    RolledBackTo {
        peer: Peer,
        rollback_header: BlockHeader,
        #[serde(skip, default = "Span::none")]
        span: Span,
    },
}

impl BlockValidationResult {
    pub fn peer(&self) -> Peer {
        match self {
            BlockValidationResult::BlockValidated { peer, .. } => peer.clone(),
            BlockValidationResult::BlockValidationFailed { peer, .. } => peer.clone(),
            BlockValidationResult::RolledBackTo { peer, .. } => peer.clone(),
        }
    }
}

impl PartialEq for BlockValidationResult {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                BlockValidationResult::BlockValidated {
                    peer: p1,
                    header: hd1,
                    ..
                },
                BlockValidationResult::BlockValidated {
                    peer: p2,
                    header: hd2,
                    ..
                },
            ) => p1 == p2 && hd1 == hd2,
            (
                BlockValidationResult::BlockValidationFailed {
                    peer: p1,
                    point: pt1,
                    ..
                },
                BlockValidationResult::BlockValidationFailed {
                    peer: p2,
                    point: pt2,
                    ..
                },
            ) => p1 == p2 && pt1 == pt2,
            (
                BlockValidationResult::RolledBackTo {
                    peer: p1,
                    rollback_header: rp1,
                    ..
                },
                BlockValidationResult::RolledBackTo {
                    peer: p2,
                    rollback_header: rp2,
                    ..
                },
            ) => p1 == p2 && rp1 == rp2,
            _ => false,
        }
    }
}
