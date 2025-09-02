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

use crate::consensus::receive_header::ReceiveHeader;
use crate::consensus::select_chain::SelectChain;
use crate::consensus::store_header::StoreHeader;
use crate::consensus::upstream_errors::UpstreamErrors;
use crate::consensus::validate_header::{ValidateHeader, ValidateHeaderState};
use crate::{ConsensusError, consensus::select_chain::SelectChainState, is_header::IsHeader};
use amaru_kernel::{Header, Point, peer::Peer, protocol_parameters::GlobalParameters};
use pure_stage::{StageGraph, StageRef};
use std::fmt;
use tracing::Span;

pub mod headers_tree;
pub mod receive_header;
pub mod select_chain;
pub mod store;
pub mod store_block;
pub mod store_effects;
pub mod store_header;
pub mod tip;
mod upstream_errors;
pub mod validate_header;

pub const EVENT_TARGET: &str = "amaru::consensus";

pub fn build_stage_graph(
    global_parameters: &GlobalParameters,
    consensus: ValidateHeaderState,
    chain_selector: SelectChainState,
    network: &mut impl StageGraph,
    outputs: StageRef<ValidateHeaderEvent>,
) -> StageRef<ChainSyncEvent> {
    let upstream_errors = network.make_stage(UpstreamErrors::name());
    let receive_header = network.make_stage(ReceiveHeader::name());
    let store_header = network.make_stage(StoreHeader::name());
    let validate_header = network.make_stage(ValidateHeader::name());
    let select_chain = network.make_stage(SelectChain::name());

    let upstream_errors =
        network.register(upstream_errors::new(network.make_name("upstream_errors")));

    network.register(select_chain, SelectChain::new(
        chain_selector,
        outputs,
        upstream_errors.as_ref(),
    ));

    let validate_header = network.register(ValidateHeader::new(
        "validate_header",
        global_parameters,
        consensus,
        select_chain.as_ref(),
        upstream_errors.as_ref(),
    ));

    let store_header_stage =
        network.register(StoreHeader::new("store_header", validate_header.as_ref()));

    network
        .register(ReceiveHeader::new(
            network.make_name("receive_header"),
            store_header_stage.as_ref(),
            upstream_errors.as_ref(),
        ))
        .as_ref()
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ValidationFailed {
    pub peer: Peer,
    pub point: Point,
    pub error: ConsensusError,
}

impl ValidationFailed {
    pub fn new(peer: Peer, point: Point, error: ConsensusError) -> Self {
        Self { peer, point, error }
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

    pub fn point(&self) -> Point {
        match self {
            DecodedChainSyncEvent::RollForward { point, .. } => point.clone(),
            DecodedChainSyncEvent::Rollback { rollback_point, .. } => rollback_point.clone(),
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
