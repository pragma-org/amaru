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

use crate::{ConsensusError, is_header::IsHeader};
use amaru_kernel::{Header, Point, peer::Peer};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fmt::Display;
use thiserror::Error;
use tracing::Span;

pub mod block_effects;
pub mod fetch_block;
pub mod headers_tree;
pub mod receive_header;
pub mod select_chain;
pub mod span;
pub mod store;
pub mod store_block;
pub mod store_effects;
pub mod store_header;
pub mod tip;
pub mod validate_block;
pub mod validate_header;

pub const EVENT_TARGET: &str = "amaru::consensus";

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize, Error)]
pub struct ValidationFailed {
    pub peer: Peer,
    pub error: ConsensusError,
}

impl Display for ValidationFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "validation failed for peer {}: {}",
            self.peer.name, self.error
        )
    }
}

impl ValidationFailed {
    pub fn new(peer: &Peer, error: ConsensusError) -> Self {
        Self {
            peer: peer.clone(),
            error,
        }
    }
}

#[derive(Debug, Error)]
pub struct ProcessingFailed {
    pub peer: Peer,
    pub error: anyhow::Error,
}

impl PartialEq for ProcessingFailed {
    fn eq(&self, other: &Self) -> bool {
        self.peer == other.peer && format!("{}", self.error) == format!("{}", other.error)
    }
}

impl Serialize for ProcessingFailed {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut state = serializer.serialize_struct("ProcessingFailed", 2)?;
        state.serialize_field("peer", &self.peer)?;
        state.serialize_field("error", &self.error.to_string())?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for ProcessingFailed {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct ProcessingFailedHelper {
            peer: Peer,
            error: String,
        }

        let helper = ProcessingFailedHelper::deserialize(deserializer)?;
        Ok(ProcessingFailed {
            peer: helper.peer,
            error: anyhow::anyhow!(helper.error),
        })
    }
}

impl Display for ProcessingFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "validation failed for peer {}: {}",
            self.peer.name, self.error
        )
    }
}

impl ProcessingFailed {
    pub fn new(peer: &Peer, error: anyhow::Error) -> Self {
        Self {
            peer: peer.clone(),
            error,
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
    CaughtUp {
        peer: Peer,
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
            ChainSyncEvent::CaughtUp { peer, .. } => f
                .debug_struct("CaughtUp")
                .field("peer", &peer.name)
                .finish(),
        }
    }
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum DecodedChainSyncEvent {
    RollForward {
        peer: Peer,
        point: Point,
        header: Header,
        #[serde(skip, default = "Span::none")]
        span: Span,
    },
    Rollback {
        peer: Peer,
        rollback_point: Point,
        #[serde(skip, default = "Span::none")]
        span: Span,
    },
    CaughtUp {
        peer: Peer,
        #[serde(skip, default = "Span::none")]
        span: Span,
    },
}

impl DecodedChainSyncEvent {
    pub fn peer(&self) -> Peer {
        match self {
            DecodedChainSyncEvent::RollForward { peer, .. } => peer.clone(),
            DecodedChainSyncEvent::Rollback { peer, .. } => peer.clone(),
            DecodedChainSyncEvent::CaughtUp { peer, .. } => peer.clone(),
        }
    }
}

impl fmt::Debug for DecodedChainSyncEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodedChainSyncEvent::RollForward {
                peer,
                point,
                header,
                ..
            } => f
                .debug_struct("RollForward")
                .field("peer", &peer.name)
                .field("point", &point.to_string())
                .field("header", &header.hash().to_string())
                .finish(),
            DecodedChainSyncEvent::Rollback {
                peer,
                rollback_point,
                ..
            } => f
                .debug_struct("Rollback")
                .field("peer", &peer.name)
                .field("rollback_point", &rollback_point.to_string())
                .finish(),
            DecodedChainSyncEvent::CaughtUp { peer, .. } => f
                .debug_struct("CaughtUp")
                .field("peer", &peer.name)
                .finish(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum ValidateHeaderEvent {
    Validated {
        peer: Peer,
        header: Header,
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
