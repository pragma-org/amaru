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

use crate::consensus;
use amaru_kernel::peer::Peer;
use amaru_kernel::{HEADER_HASH_SIZE, Point};
use amaru_ouroboros::praos::header::AssertHeaderError;
use amaru_ouroboros_traits::StoreError;
use pallas_crypto::hash::Hash;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fmt::Display;
use thiserror::Error;

#[derive(Error, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ConsensusError {
    #[error("cannot build a chain selector without a tip")]
    MissingTip,
    #[error("Failed to fetch block at {0}")]
    FetchBlockFailed(Point),
    #[error("Failed to validate header at {0}: {1}")]
    InvalidHeader(Point, AssertHeaderError),
    #[error("Failed to store header at {0}: {1}")]
    StoreHeaderFailed(Hash<HEADER_HASH_SIZE>, StoreError),
    #[error("Failed to remove header at {0}: {1}")]
    RemoveHeaderFailed(Hash<HEADER_HASH_SIZE>, StoreError),
    #[error("Failed to set a new anchor at {0}: {1}")]
    SetAnchorHashFailed(Hash<HEADER_HASH_SIZE>, StoreError),
    #[error("Failed to set a best chain at {0}: {1}")]
    SetBestChainHashFailed(Hash<HEADER_HASH_SIZE>, StoreError),
    #[error("Failed to update a best chain at {0}->{1}: {2}")]
    UpdateBestChainFailed(Hash<HEADER_HASH_SIZE>, Hash<HEADER_HASH_SIZE>, StoreError),
    #[error("Failed to store block body at {0}: {1}")]
    StoreBlockFailed(Point, StoreError),
    #[error(
        "Failed to decode header at {}: {} ({})",
        point,
        hex::encode(header),
        reason
    )]
    CannotDecodeHeader {
        point: Point,
        header: Vec<u8>,
        reason: String,
    },
    #[error("Unknown peer {0}, bailing out")]
    UnknownPeer(Peer),
    #[error("Unknown point {0}, bailing out")]
    UnknownPoint(Hash<HEADER_HASH_SIZE>),
    #[error(
        "Invalid rollback {} from peer {}, cannot go further than {}",
        rollback_point,
        peer,
        max_point
    )]
    InvalidRollback {
        peer: Peer,
        rollback_point: Hash<HEADER_HASH_SIZE>,
        max_point: Hash<HEADER_HASH_SIZE>,
    },
    #[error("Invalid block from peer {} at {}", peer, point)]
    InvalidBlock { peer: Peer, point: Point },
    #[error("{0}")]
    NoncesError(#[from] consensus::store::NoncesError),
    #[error("{0}")]
    InvalidHeaderParent(Box<InvalidHeaderParentData>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct InvalidHeaderParentData {
    pub(crate) peer: Peer,
    pub(crate) forwarded: Point,
    pub(crate) actual: Option<Hash<HEADER_HASH_SIZE>>,
    pub(crate) expected: Point,
}

impl Display for InvalidHeaderParentData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Invalid forwarded header {} from peer {}, actual parent {:?}, expected parent {}",
            self.forwarded, self.peer, self.actual, self.expected
        )
    }
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
            "validation failed for peer {}: {}",
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
