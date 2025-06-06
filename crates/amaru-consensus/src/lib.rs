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
// limitations under the License

use amaru_kernel::Point;
use amaru_ouroboros::praos::header::AssertHeaderError;
use thiserror::Error;

pub use amaru_ouroboros_traits::*;

/// Consensus interface
///
/// The consensus interface is responsible for validating block headers.
pub mod consensus;

pub mod peer;

pub type RawHeader = Vec<u8>;

#[derive(Error, Debug)]
pub enum ConsensusError {
    #[error("cannot build a chain selector without a tip")]
    MissingTip,
    #[error("Failed to fetch block at {0:?}")]
    FetchBlockFailed(Point),
    #[error("Failed to validate header at {0:?}: {1}")]
    InvalidHeader(Point, AssertHeaderError),
    #[error("Failed to store header at {0:?}: {1}")]
    StoreHeaderFailed(Point, consensus::store::StoreError),
    #[error("Failed to store block body at {0:?}: {1}")]
    StoreBlockFailed(Point, consensus::store::StoreError),
    #[error("Failed to decode header at {}: {}", point, hex::encode(header))]
    CannotDecodeHeader { point: Point, header: Vec<u8> },
    #[error("Unknown peer {0:?}, bailing out")]
    UnknownPeer(peer::Peer),
    #[error("{0}")]
    NoncesError(#[from] consensus::store::NoncesError),
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
