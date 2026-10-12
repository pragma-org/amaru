// Copyright 2026 PRAGMA
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

//! Block-fetch selection and the request, ask, and failure records around it.

#![expect(clippy::unit_arg)]

use amaru_kernel::{HeaderHash, Peer};
use amaru_pure_stage::{BoxFuture, ExternalEffectAPI, Instant, Resources, SendData};

use super::{enqueue, enqueue_and_emit_telemetry, enqueue_query, require_perf};
use crate::performance::{
    ClaimKind, FetchPeerSet, Performance, SelectPeersParams,
    ops::{HeaderOp, PeerOp, PerformanceOp},
};

impl Performance {
    /// Record that `peers` were asked for `hashes` at `at`, and emit `block.requested`.
    ///
    /// The first ask of a peer is kept. A later ask of a different peer, as in a staggered retry,
    /// records its own time.
    pub fn record_peers_asked(hashes: Vec<HeaderHash>, peers: Vec<Peer>, at: Instant) -> RecordPeersAskedEffect {
        RecordPeersAskedEffect { hashes, peers, at }
    }

    pub fn record_blocks_requested(hashes: Vec<HeaderHash>, requested_at: Instant) -> RecordBlocksRequestedEffect {
        RecordBlocksRequestedEffect { hashes, requested_at }
    }

    pub fn record_fetch_failure(peers: Vec<Peer>, at: Instant) -> RecordFetchFailureEffect {
        RecordFetchFailureEffect { peers, at }
    }

    pub fn select_peers_for_fetch(params: SelectPeersParams) -> SelectPeersForFetchEffect {
        SelectPeersForFetchEffect { params }
    }

    pub fn peer_covers_fragment(peer: Peer, need: Vec<HeaderHash>) -> PeerCoversFragmentEffect {
        PeerCoversFragmentEffect { peer, need }
    }

    pub fn direct_claimants(hash: HeaderHash) -> DirectClaimantsEffect {
        DirectClaimantsEffect { hash }
    }

    pub fn first_announced_at(hash: HeaderHash) -> FirstAnnouncedAtEffect {
        FirstAnnouncedAtEffect { hash }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordPeersAskedEffect {
    pub(crate) hashes: Vec<HeaderHash>,
    pub(crate) peers: Vec<Peer>,
    pub(crate) at: Instant,
}

impl ExternalEffectAPI for RecordPeersAskedEffect {
    type Response = ();

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        let resources = resources.clone();
        self.wrap(|this| async move {
            enqueue_and_emit_telemetry(&perf, resources, |reply| {
                PerformanceOp::Header(HeaderOp::RecordPeersAsked { effect: this, reply })
            })
            .await
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordBlocksRequestedEffect {
    pub(crate) hashes: Vec<HeaderHash>,
    pub(crate) requested_at: Instant,
}

impl ExternalEffectAPI for RecordBlocksRequestedEffect {
    type Response = ();

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let perf = require_perf(&resources);
            enqueue(&perf, PerformanceOp::Header(HeaderOp::RecordBlocksRequested { effect: self.as_ref().clone() }));
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordFetchFailureEffect {
    pub(crate) peers: Vec<Peer>,
    pub(crate) at: Instant,
}

impl ExternalEffectAPI for RecordFetchFailureEffect {
    type Response = ();

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let perf = require_perf(&resources);
            enqueue(&perf, PerformanceOp::Peer(PeerOp::RecordFetchFailure { effect: self.as_ref().clone() }));
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SelectPeersForFetchEffect {
    pub(crate) params: SelectPeersParams,
}

impl ExternalEffectAPI for SelectPeersForFetchEffect {
    type Response = FetchPeerSet;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        self.wrap(|this| async move {
            enqueue_query(&perf, |reply| PerformanceOp::Peer(PeerOp::SelectPeersForFetch { effect: this, reply })).await
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PeerCoversFragmentEffect {
    pub(crate) peer: Peer,
    pub(crate) need: Vec<HeaderHash>,
}

impl ExternalEffectAPI for PeerCoversFragmentEffect {
    type Response = bool;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        self.wrap(|this| async move {
            enqueue_query(&perf, |reply| PerformanceOp::Peer(PeerOp::PeerCoversFragment { effect: this, reply })).await
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DirectClaimantsEffect {
    pub(crate) hash: HeaderHash,
}

impl ExternalEffectAPI for DirectClaimantsEffect {
    type Response = Vec<(Peer, Instant, ClaimKind)>;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        self.wrap(|this| async move {
            enqueue_query(&perf, |reply| PerformanceOp::Peer(PeerOp::DirectClaimants { effect: this, reply })).await
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FirstAnnouncedAtEffect {
    pub(crate) hash: HeaderHash,
}

impl ExternalEffectAPI for FirstAnnouncedAtEffect {
    type Response = Option<(Peer, Instant)>;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        self.wrap(|this| async move {
            enqueue_query(&perf, |reply| PerformanceOp::Peer(PeerOp::FirstAnnouncedAt { effect: this, reply })).await
        })
    }
}
