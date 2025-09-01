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
use crate::consensus::validate_header::ValidateHeader;
use crate::{ConsensusError, consensus::select_chain::SelectChainState, is_header::IsHeader};
use amaru_kernel::{Header, Point, peer::Peer, protocol_parameters::GlobalParameters};
use amaru_ouroboros_traits::HasStakeDistribution;
use pure_stage::{StageGraph, StageRef};
use std::fmt;
use std::sync::Arc;
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

pub fn build_consensus_stages(
    global_parameters: &GlobalParameters,
    ledger: Arc<dyn HasStakeDistribution>,
    chain_selector: SelectChainState,
    network: &mut impl StageGraph,
    outputs: impl AsRef<StageRef<ValidateHeaderEvent>>,
) -> StageRef<ChainSyncEvent> {
    let upstream_errors = network.make_stage("upstream_errors");
    let receive_header = network.make_stage("receive_header");
    let store_header = network.make_stage("store_header");
    let validate_header = network.make_stage("validate_header");
    let select_chain = network.make_stage("select_chain");

    let upstream_errors = network.register(&upstream_errors, UpstreamErrors);
    network.register(
        &receive_header,
        ReceiveHeader::new(&store_header, &upstream_errors),
    );
    network.register(&store_header, StoreHeader::new(&validate_header));
    network.register(
        &validate_header,
        ValidateHeader::new(global_parameters, ledger, &select_chain, &upstream_errors),
    );
    network.register(
        &select_chain,
        SelectChain::new(chain_selector, outputs, &upstream_errors),
    );
    receive_header.without_state()
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
