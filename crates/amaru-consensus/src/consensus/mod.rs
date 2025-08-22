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

use crate::{ConsensusError, consensus::select_chain::SelectChain, is_header::IsHeader};
use amaru_kernel::{Header, Point, peer::Peer, protocol_parameters::GlobalParameters};
use pure_stage::{StageGraph, StageRef, Void};
use tracing::Span;

pub mod headers_tree;
pub mod receive_header;
pub mod select_chain;
pub mod store;
pub mod store_block;
pub mod store_effects;
pub mod store_header;
pub mod tip;
pub mod validate_header;

pub const EVENT_TARGET: &str = "amaru::consensus";

pub fn build_stage_graph(
    global_parameters: &GlobalParameters,
    consensus: validate_header::ValidateHeader,
    chain_selector: SelectChain,
    network: &mut impl StageGraph,
    outputs: StageRef<ValidateHeaderEvent, Void>,
) -> StageRef<ChainSyncEvent, Void> {
    let receive_header_stage = network.stage("receive_header", receive_header::stage);
    let store_header_stage = network.stage("store_header", store_header::stage);
    let validate_header_stage = network.stage("validate_header", validate_header::stage);
    let select_chain_stage = network.stage("select_chain", select_chain::stage);

    // TODO: currently only valiate_header errors, will need to grow into all error handling
    let upstream_errors_stage = network.stage("upstream_errors", async |_, msg, eff| {
        let ValidationFailed { peer, error } = msg;
        tracing::error!(%peer, %error, "invalid header");

        // TODO: implement specific actions once we have an upstream network

        // termination here will tear down the entire stage graph
        eff.terminate().await
    });

    let upstream_errors_stage = network.wire_up(upstream_errors_stage, ());

    let select_chain_stage = network.wire_up(
        select_chain_stage,
        (
            chain_selector,
            outputs.without_state(),
            upstream_errors_stage.without_state(),
        ),
    );

    let validate_header_stage = network.wire_up(
        validate_header_stage,
        (
            consensus,
            global_parameters.clone(),
            select_chain_stage.without_state(),
            upstream_errors_stage.without_state(),
        ),
    );

    let store_header_stage =
        network.wire_up(store_header_stage, validate_header_stage.without_state());

    let receive_header_stage = network.wire_up(
        receive_header_stage,
        (
            store_header_stage.without_state(),
            upstream_errors_stage.without_state(),
        ),
    );

    receive_header_stage.without_state()
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ValidationFailed {
    pub peer: Peer,
    pub error: ConsensusError,
}

impl ValidationFailed {
    pub fn new(peer: Peer, error: ConsensusError) -> Self {
        Self { peer, error }
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

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
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
