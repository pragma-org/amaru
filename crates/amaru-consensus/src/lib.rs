#![feature(assert_matches)]
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
#![deny(clippy::future_not_send)]

use amaru_kernel::{HEADER_HASH_SIZE, Point, peer::Peer};
use amaru_ouroboros::praos::header::AssertHeaderError;
use pallas_crypto::hash::Hash;
use std::fmt;
use std::fmt::Display;
use thiserror::Error;

pub use amaru_ouroboros_traits::*;

/// Consensus interface
///
/// The consensus interface is responsible for validating block headers.
pub mod consensus;
pub mod span;

pub type RawHeader = Vec<u8>;

#[derive(Error, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ConsensusError {
    #[error("cannot build a chain selector without a tip")]
    MissingTip,
    #[error("Failed to fetch block at {0}")]
    FetchBlockFailed(Point),
    #[error("Failed to validate header at {0}: {1}")]
    InvalidHeader(Point, AssertHeaderError),
    #[error("Failed to store header at {0}: {1}")]
    StoreHeaderFailed(Point, consensus::store::StoreError),
    #[error("Failed to store block body at {0}: {1}")]
    StoreBlockFailed(Point, consensus::store::StoreError),
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
    #[error("{0}")]
    NoncesError(#[from] consensus::store::NoncesError),
    #[error("{0}")]
    InvalidHeaderParent(Box<InvalidHeaderParentData>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct InvalidHeaderParentData {
    peer: Peer,
    forwarded: Point,
    actual: Option<Hash<HEADER_HASH_SIZE>>,
    expected: Point,
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

#[cfg(test)]
pub(crate) mod test {
    macro_rules! include_header {
        ($name:ident, $slot:expr) => {
            static $name: std::sync::LazyLock<Header> = std::sync::LazyLock::new(|| {
                let data =
                    include_bytes!(concat!("../../tests/data/headers/preprod_", $slot, ".cbor"));
                amaru_kernel::from_cbor(data.as_slice()).expect("invalid header")
            });
        };
    }

    pub(crate) use include_header;
}
