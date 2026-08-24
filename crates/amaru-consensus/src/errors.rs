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

use std::{fmt, fmt::Display};

use amaru_kernel::{BlockHeight, EraName, HeaderHash, Peer, Point, PoolId, Slot};
use amaru_ouroboros_traits::{StoreError, has_stake_distribution::GetPoolError};
use serde::ser::SerializeStruct;
use thiserror::Error;

use crate::validate_header::ValidateHeaderError;

#[derive(Error, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ConsensusError {
    #[error("cannot build a chain selector without a tip")]
    MissingTip,
    #[error("Failed to fetch block at {0}")]
    FetchBlockFailed(Point),
    #[error("Failed to validate header at {0}: {1}")]
    InvalidHeader(Point, Box<ValidateHeaderError>),
    #[error("Failed to store header at {0}: {1}")]
    StoreHeaderFailed(HeaderHash, StoreError),
    #[error("Failed to remove header at {0}: {1}")]
    RemoveHeaderFailed(HeaderHash, StoreError),
    #[error("Failed to update a best chain at {0}->{1}: {2}")]
    UpdateBestChainFailed(HeaderHash, HeaderHash, StoreError),
    #[error("Failed to store block body at {0}: {1}")]
    StoreBlockFailed(Point, StoreError),
    #[error("Header point {} does not match expected point {}", actual_point, expected_point)]
    HeaderPointMismatch { actual_point: Point, expected_point: Point },
    #[error("Failed to decode header: {} ({})",
        hex::encode(&header[..header.len().min(32)]),
        reason
    )]
    CannotDecodeHeader { header: Vec<u8>, reason: String },
    #[error("Unknown peer {0}, bailing out")]
    UnknownPeer(Peer),
    #[error("Unknown point {0}, bailing out")]
    UnknownPoint(HeaderHash),
    #[error("Invalid rollback {} from peer {}, cannot go further than {}", rollback_point, peer, max_point)]
    InvalidRollback { peer: Peer, rollback_point: HeaderHash, max_point: HeaderHash },
    #[error("Invalid block from peer {} at {}", peer, point)]
    InvalidBlock { peer: Peer, point: Point },
    #[error("Invalid block at {} build on invalid block {}", point, invalid)]
    BlockBuiltOnInvalidBlock { point: Point, invalid: Point },
    #[error("{0}")]
    NoncesError(#[from] crate::store::NoncesError),
    #[error("{0}")]
    InvalidHeaderParent(Box<InvalidHeaderParentData>),
    #[error("Invalid header height {actual}, expected {expected}")]
    InvalidHeaderHeight { actual: BlockHeight, expected: BlockHeight },
    #[error("{0}")]
    InvalidHeaderPoint(Box<InvalidHeaderPoint>),
    #[error("Invalid header variant {0}")]
    InvalidHeaderVariant(EraName),
    #[error("header slot {0} is in the near future (permissible clock skew)")]
    HeaderSlotInNearFuture(Slot),
    #[error("{0}")]
    HeaderSlotTooFarInFuture(Box<HeaderSlotTooFarInFuture>),
    #[error("Failed to roll forward chain from {0}: {1}")]
    RollForwardChainFailed(amaru_kernel::Hash<32>, StoreError),
    #[error("Failed to rollback chain at {0}: {1}")]
    RollbackChainFailed(Point, StoreError),
    #[error("{0}")]
    EraHistoryError(#[from] amaru_kernel::EraHistoryError),
    #[error("Era name mismatch: from raw_header {from_raw_header}, from slot={from_slot}")]
    EraNameMismatch { from_raw_header: EraName, from_slot: EraName },
    #[error("Failed to convert issuer public key")]
    IssuerFromPublicKeyError,
    #[error("{0}")]
    StoreError(#[from] StoreError),
    #[error("{0}")]
    GetPoolError(#[from] GetPoolError),
    #[error("Unknown pool: {}", hex::encode(&pool_id[0..7]))]
    UnknownPool { pool_id: PoolId },
}

impl ConsensusError {
    pub fn as_invalid_header(&self) -> Option<&ValidateHeaderError> {
        if let ConsensusError::InvalidHeader(_, err) = self { Some(err) } else { None }
    }
    pub fn as_invalid_header_parent(&self) -> Option<&InvalidHeaderParentData> {
        if let ConsensusError::InvalidHeaderParent(err) = self { Some(err) } else { None }
    }
    pub fn as_invalid_header_point(&self) -> Option<&InvalidHeaderPoint> {
        if let ConsensusError::InvalidHeaderPoint(err) = self { Some(err) } else { None }
    }
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
#[error(
    "Invalid header point {actual}: slot does not progress from parent {parent} (upstream peer’s best validated is at {highest})"
)]
pub struct InvalidHeaderPoint {
    pub actual: Point,
    pub parent: Point,
    pub highest: Point,
}

#[derive(Error, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[error(
    "header point {actual} is {delta_millis}ms ahead of local time (slot onset {onset_millis}ms since system start, local {elapsed_millis}ms; max skew {max_skew_millis}ms; parent {parent}; peer tip {highest})"
)]
pub struct HeaderSlotTooFarInFuture {
    pub actual: Point,
    pub parent: Point,
    pub highest: Point,
    pub onset_millis: u64,
    pub elapsed_millis: u64,
    pub delta_millis: u64,
    pub max_skew_millis: u64,
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
        write!(f, "validation failed for peer {}: {}", self.peer.name, self.error)
    }
}

impl ValidationFailed {
    pub fn new(peer: &Peer, error: ConsensusError) -> Self {
        Self { peer: peer.clone(), error }
    }
}

impl From<ConsensusError> for ValidationFailed {
    fn from(error: ConsensusError) -> Self {
        Self { peer: Peer::new(""), error }
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
        Ok(ProcessingFailed { peer: helper.peer, error: anyhow::anyhow!(helper.error) })
    }
}

impl Display for ProcessingFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "processing failed for peer {}: {}",
            self.peer.clone().map(|p| p.name).unwrap_or("n/a".to_string()),
            self.error
        )
    }
}

impl ProcessingFailed {
    pub fn new(peer: &Peer, error: anyhow::Error) -> Self {
        Self { peer: Some(peer.clone()), error }
    }

    pub fn from(error: anyhow::Error) -> Self {
        Self { peer: None, error }
    }
}

#[cfg(test)]
mod logged_error_text {
    use amaru_kernel::{BlockHeight, Hash, Point, Slot};

    use super::{ConsensusError, HeaderSlotTooFarInFuture};
    use crate::{store::NoncesError, validate_header::ValidateHeaderError};

    fn sample_point(height: u64) -> Point {
        Point::Specific(Slot::from(42), Hash::new([0xabu8; 32]), BlockHeight::from(height))
    }

    /// Header-lifecycle traces stringify `ConsensusError` because the schema lives in
    /// `amaru-observability` and cannot name this crate's error enum. Display must still
    /// carry the point and the distinguishing numbers so the log line is usable.
    #[test]
    fn header_clock_skew_display_includes_point_and_delta() {
        let actual = sample_point(7);
        let error = ConsensusError::HeaderSlotTooFarInFuture(Box::new(HeaderSlotTooFarInFuture {
            actual,
            parent: Point::Origin,
            highest: Point::Origin,
            onset_millis: 1_000,
            elapsed_millis: 100,
            delta_millis: 900,
            max_skew_millis: 2_000,
        }));

        let text = error.to_string();
        assert!(text.contains(&actual.to_string()), "display must include the header point: {text}");
        assert!(text.contains("900ms"), "display must include the skew delta: {text}");
    }

    /// Same check for a variant that wraps another error type (not only a boxed payload).
    #[test]
    fn nested_header_validation_display_includes_point_and_inner_error() {
        let actual = sample_point(7);
        let header = actual.hash();
        let inner = ValidateHeaderError::Nonces(NoncesError::UnknownHeader { header });
        let error = ConsensusError::InvalidHeader(actual, Box::new(inner));

        let text = error.to_string();
        assert!(text.contains(&actual.to_string()), "display must include the header point: {text}");
        assert!(text.contains(&header.to_string()), "display must include the inner unknown-header hash: {text}");
        assert!(text.contains("evolve_nonce failed"), "display must include the inner error: {text}");
        assert!(text.contains("evolve_nonce failed"), "display must include the inner error: {text}");
        assert!(text.contains("unknown header"), "display must include the inner error: {text}");
    }
}
