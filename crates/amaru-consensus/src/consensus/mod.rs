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

use std::fmt;

use crate::{consensus::validate_header::ValidationFailed, is_header::IsHeader};
use amaru_kernel::{Header, Point, peer::Peer, protocol_parameters::GlobalParameters};
use pure_stage::{StageGraph, StageRef, Void};
use serde::{Deserialize, Serialize};
use tracing::Span;

pub mod headers_tree;
pub mod receive_header;
pub mod select_chain;
pub mod store;
pub mod store_block;
pub mod store_header;
pub mod tip;
pub mod validate_header;

pub const EVENT_TARGET: &str = "amaru::consensus";

pub fn build_stage_graph(
    global_parameters: &GlobalParameters,
    consensus: validate_header::ValidateHeader,
    network: &mut impl StageGraph,
    validation_outputs: StageRef<DecodedChainSyncEvent, Void>,
) -> StageRef<DecodedChainSyncEvent, Void> {
    let validate_header_stage = network.stage("validate_header", validate_header::stage);

    let errors_stage = network.stage("errors", async |_, msg, eff| {
        let ValidationFailed {
            peer,
            error,
            action,
        } = msg;
        tracing::error!(%peer, %error, ?action, "invalid header");

        // TODO: implement specific actions once we have an upstream network

        // termination here will tear down the entire stage graph
        eff.terminate().await
    });

    let errors_stage = network.wire_up(errors_stage, ());

    let validate_header_stage = network.wire_up(
        validate_header_stage,
        (
            consensus,
            global_parameters.clone(),
            validation_outputs,
            errors_stage.without_state(),
        ),
    );

    validate_header_stage.without_state()
}

#[derive(Clone)]
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

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[allow(clippy::large_enum_variant)]
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
}

impl DecodedChainSyncEvent {
    pub fn peer(&self) -> Peer {
        match self {
            DecodedChainSyncEvent::RollForward { peer, .. } => peer.clone(),
            DecodedChainSyncEvent::Rollback { peer, .. } => peer.clone(),
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
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[allow(clippy::large_enum_variant)]
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

#[cfg(any(test, feature = "test-utils"))]
pub mod generators {
    use amaru_ouroboros::fake::FakeHeader;
    use rand::{RngCore, SeedableRng, rngs::StdRng};
    use rand_distr::{Distribution, Exp};

    use super::*;

    /// Very simple function to generate random sequence of bytes of given length.
    pub fn random_bytes(arg: u32) -> Vec<u8> {
        let mut rng = StdRng::from_os_rng();
        let mut buffer = vec![0; arg as usize];
        rng.fill_bytes(&mut buffer);
        buffer
    }

    /// Generate a chain of headers anchored at a given header.
    ///
    /// The chain is generated by creating headers with random body hash, and linking
    /// them to the previous header in the chain until the desired length is reached.
    #[allow(clippy::unwrap_used)]
    pub fn generate_headers_anchored_at(
        anchor: Option<FakeHeader>,
        length: usize,
    ) -> Vec<FakeHeader> {
        let mut headers: Vec<FakeHeader> = Vec::new();
        let mut parent = anchor;
        // simulate block distribution on mainnet as an exponential distribution with
        // parameter λ = 1/20
        let poi = Exp::new(0.05).unwrap();
        let mut rng = rand::rng();
        for _ in 0..length {
            let next_slot: f32 = poi.sample(&mut rng);
            let header = FakeHeader {
                block_number: parent.map_or(0, |h| h.block_height()) + 1,
                slot: parent.map_or(0, |h| h.slot()) + (next_slot.floor() as u64),
                parent: parent.map(|h| h.hash()),
                body_hash: random_bytes(32).as_slice().into(),
            };
            headers.push(header);
            parent = Some(header);
        }
        headers
    }
}
