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
use opentelemetry::metrics::MeterProvider;
use opentelemetry::metrics::{Counter, Gauge};
use opentelemetry::KeyValue;
use opentelemetry_sdk::metrics::SdkMeterProvider;
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

#[derive(Debug, Clone)]
pub struct ConsensusMetrics {
    /// The current slot for tip for this node.
    pub current_tip_slot: Gauge<u64>,

    /// The number of headers kept in the `ChainSelector`
    pub chain_selection_size: Gauge<u64>,

    /// The length of the longest fragment held in the `ChainSelector`
    pub max_fragment_length: Gauge<u64>,

    // NOTE: following counters track each of the consensus' stages
    /// The number of `RollForward` messages received from upstream peers
    pub count_forwards_received: Counter<u64>,

    /// The number of `Rollback` messages received from upstream peers
    pub count_rollbacks_received: Counter<u64>,

    /// The number of headers stored
    pub count_stored_headers: Counter<u64>,

    /// The number of headers validated
    pub count_validated_headers: Counter<u64>,

    /// The number of blocks fetched
    pub count_fetched_blocks: Counter<u64>,

    /// The accumulated count of bytes stored for blocks
    pub count_stored_blocks_bytes: Counter<u64>,

    /// The number of blocks validated
    pub count_validated_blocks: Counter<u64>,

    /// The number of forward headers propagated
    pub count_forwarded_headers: Counter<u64>,

    // NOTE: counters for data received/sent
    /// The number of headers sent to peers as a reply to
    /// `RequestNext`.
    pub count_sent_headers: Counter<u64>,

    /// The number of blocks sent to peers
    pub count_sent_block: Counter<u64>,
}

/// Useful constant where no metadata is passed for metrics
pub const NO_KEY_VALUE: [KeyValue; 0] = [];

impl ConsensusMetrics {
    pub fn new(metrics: &mut SdkMeterProvider) -> Self {
        let meter = metrics.meter("consensus");
        let current_tip_slot = meter
            .u64_gauge("current_tip_slot")
            .with_description("The slot of the current tip of this node")
            .with_unit("Slot")
            .build();

        let chain_selection_size = meter
            .u64_gauge("chain_selection_size")
            .with_description("The total number of headers kept in memory for chain selection")
            .build();

        let max_fragment_length = meter
            .u64_gauge("max_fragment_length")
            .with_description("The length of the longest fragment kept in chain selection")
            .build();

        let count_forwards_received = meter
            .u64_counter("count_forwards_received")
            .with_description("Number of RollForward messages received")
            .build();

        let count_rollbacks_received = meter
            .u64_counter("count_rollbacks_received")
            .with_description("Number of RollBack messages received")
            .build();

        let stored_headers = meter
            .u64_counter("count_stored_headers")
            .with_description("Number of write operations for storing headers on disk")
            .build();

        let count_validated_headers = meter
            .u64_counter("count_validated_headers")
            .with_description("Number of headers validated")
            .build();

        let count_fetched_blocks = meter
            .u64_counter("count_fetched_headers")
            .with_description("Number of headers fetched from peers")
            .build();

        let count_stored_blocks_bytes = meter
            .u64_counter("count_stored_blocks_bytes")
            .with_description("Number of bytes written storing blocks on disk")
            .with_unit("B")
            .build();

        let count_validated_blocks = meter
            .u64_counter("count_validated_blocks")
            .with_description("Number of blocks validated")
            .build();

        let count_forwarded_headers = meter
            .u64_counter("count_forwarded_headers")
            .with_description("Number of headers passed to forward stage")
            .build();

        let count_sent_headers = meter
            .u64_counter("count_sent_headers")
            .with_description("Number of headers sent to downstream peers")
            .build();

        let count_sent_block = meter
            .u64_counter("count_sent_headers")
            .with_description("Number of headers sent to downstream peers")
            .build();

        ConsensusMetrics {
            current_tip_slot,
            chain_selection_size,
            max_fragment_length,
            count_forwards_received,
            count_rollbacks_received,
            count_stored_headers: stored_headers,
            count_validated_headers,
            count_fetched_blocks,
            count_stored_blocks_bytes,
            count_validated_blocks,
            count_forwarded_headers,
            count_sent_headers,
            count_sent_block,
        }
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
