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

use amaru_kernel::{BlockHeight, EraName, HeaderHash, Peer, Point};
use amaru_ouroboros_traits::{
    BlockValidationError, StoreError, can_validate_blocks::HeaderValidationError,
};
use serde::ser::SerializeStruct;
use std::{fmt, fmt::Display};
use thiserror::Error;

#[derive(Error, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ConsensusError {
    #[error("cannot build a chain selector without a tip")]
    MissingTip,
    #[error("Failed to fetch block at {0}")]
    FetchBlockFailed(Point),
    #[error("Failed to validate header at {0}: {1}")]
    InvalidHeader(Point, HeaderValidationError),
    #[error("Failed to store header at {0}: {1}")]
    StoreHeaderFailed(HeaderHash, StoreError),
    #[error("Failed to remove header at {0}: {1}")]
    RemoveHeaderFailed(HeaderHash, StoreError),
    #[error("Failed to set a new anchor at {0}: {1}")]
    SetAnchorHashFailed(HeaderHash, StoreError),
    #[error("Failed to set a best chain at {0}: {1}")]
    SetBestChainHashFailed(HeaderHash, StoreError),
    #[error("Failed to update a best chain at {0}->{1}: {2}")]
    UpdateBestChainFailed(HeaderHash, HeaderHash, StoreError),
    #[error("Failed to store block body at {0}: {1}")]
    StoreBlockFailed(Point, StoreError),
    #[error(
        "Header point {} does not match expected point {}",
        actual_point,
        expected_point
    )]
    HeaderPointMismatch {
        actual_point: Point,
        expected_point: Point,
    },
    #[error("Failed to decode header: {} ({})", hex::encode(&header[..header.len().min(32)]), reason)]
    CannotDecodeHeader { header: Vec<u8>, reason: String },
    #[error("Unknown peer {0}, bailing out")]
    UnknownPeer(Peer),
    #[error("Unknown point {0}, bailing out")]
    UnknownPoint(HeaderHash),
    #[error(
        "Invalid rollback {} from peer {}, cannot go further than {}",
        rollback_point,
        peer,
        max_point
    )]
    InvalidRollback {
        peer: Peer,
        rollback_point: HeaderHash,
        max_point: HeaderHash,
    },
    #[error("Invalid block from peer {} at {}", peer, point)]
    InvalidBlock { peer: Peer, point: Point },
    #[error("{0}")]
    NoncesError(#[from] crate::store::NoncesError),
    #[error("{0}")]
    InvalidHeaderParent(Box<InvalidHeaderParentData>),
    #[error("Invalid header height {actual}, expected {expected}")]
    InvalidHeaderHeight {
        actual: BlockHeight,
        expected: BlockHeight,
    },
    #[error("{0}")]
    InvalidHeaderPoint(Box<InvalidHeaderPoint>),
    #[error("Invalid header variant {0}")]
    InvalidHeaderVariant(EraName),
    #[error("Failed to roll forward chain from {0}: {1}")]
    RollForwardChainFailed(amaru_kernel::Hash<32>, StoreError),
    #[error("Failed to rollback chain at {0}: {1}")]
    RollbackChainFailed(Point, StoreError),
    #[error("Failed to rollback block at {0}: {1}")]
    RollbackBlockFailed(Point, BlockValidationError),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct InvalidHeaderParentData {
    pub(crate) peer: Peer,
    pub(crate) forwarded: Point,
    pub(crate) actual: Option<HeaderHash>,
    pub(crate) expected: Point,
}

impl Display for InvalidHeaderParentData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Invalid header parent at {} from peer {}, actual parent {:?}, expected parent {}",
            self.forwarded, self.peer, self.actual, self.expected
        )
    }
}

#[derive(Error, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[error("Invalid header point {actual}, expected window ({parent}, {highest}]")]
pub struct InvalidHeaderPoint {
    pub actual: Point,
    pub parent: Point,
    pub highest: Point,
}

/// A ValidationFailed error is raised when some incoming data is invalid
/// according to the consensus rules.
/// This is not a fatal error, and should be handled gracefully.
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

/// A ProcessingFailed error is raised when some internal processing
/// fails due to an unexpected error (e.g. database error).
#[derive(Debug, Error)]
pub struct ProcessingFailed {
    pub peer: Option<Peer>,
    pub error: anyhow::Error,
}

impl PartialEq for ProcessingFailed {
    fn eq(&self, other: &Self) -> bool {
        self.peer == other.peer && format!("{}", self.error) == format!("{}", other.error)
    }
}

impl serde::Serialize for ProcessingFailed {
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

impl<'de> serde::Deserialize<'de> for ProcessingFailed {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct ProcessingFailedHelper {
            peer: Option<Peer>,
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
            "processing failed for peer {}: {}",
            self.peer
                .clone()
                .map(|p| p.name)
                .unwrap_or("n/a".to_string()),
            self.error
        )
    }
}

impl ProcessingFailed {
    pub fn new(peer: &Peer, error: anyhow::Error) -> Self {
        Self {
            peer: Some(peer.clone()),
            error,
        }
    }

    pub fn from(error: anyhow::Error) -> Self {
        Self { peer: None, error }
    }
}
