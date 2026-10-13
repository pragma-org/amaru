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

//! Outbound mix selection, churn ranking, and candidate-pool maintenance.

#![expect(clippy::unit_arg)]

use std::collections::BTreeSet;

use amaru_kernel::{Peer, PeerCandidate};
use amaru_pure_stage::{BoxFuture, ExternalEffectAPI, Instant, Resources, SendData};

use super::{enqueue, enqueue_query, require_perf};
use crate::performance::{
    PeerScores, PeerView, Performance, SelectOutboundParams, SelectUsing,
    ops::{PeerOp, PerformanceOp},
    peers::{rank_churn, select_outbound_from},
};

impl Performance {
    pub fn rank_peers_for_churn(candidates: Vec<Peer>, now: Instant) -> RankPeersForChurnEffect {
        RankPeersForChurnEffect { candidates, now }
    }

    pub fn scores(peer: Peer) -> ScoresEffect {
        ScoresEffect { peer }
    }

    pub fn set_ledger_candidates(candidates: std::collections::BTreeSet<PeerCandidate>) -> SetLedgerCandidatesEffect {
        SetLedgerCandidatesEffect { candidates }
    }

    pub fn select_outbound(params: crate::performance::SelectOutboundParams) -> SelectOutboundEffect {
        SelectOutboundEffect { params }
    }

    pub fn query_peer_view(since_generation: u64) -> QueryPeerViewEffect {
        QueryPeerViewEffect { since_generation }
    }

    pub fn is_static_peer(peer: Peer) -> IsStaticPeerEffect {
        IsStaticPeerEffect { peer }
    }

    pub fn note_dial(
        origin: crate::performance::PeerSource,
        candidate: PeerCandidate,
        peer: Peer,
        at: Instant,
    ) -> NoteDialEffect {
        NoteDialEffect { origin, candidate, peer, at }
    }

    /// Enqueue one retention batch. The caller does not wait for the worker to finish it.
    pub fn evict_records(
        now: Instant,
        protected_peers: BTreeSet<Peer>,
        protected_candidates: BTreeSet<PeerCandidate>,
    ) -> EvictRecordsEffect {
        EvictRecordsEffect { now, protected_peers, protected_candidates }
    }

    pub fn shared_contains(peer: Peer) -> SharedContainsEffect {
        SharedContainsEffect { peer }
    }

    pub fn source_counts() -> SourceCountsEffect {
        SourceCountsEffect
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RankPeersForChurnEffect {
    pub(crate) candidates: Vec<Peer>,
    pub(crate) now: Instant,
}

impl ExternalEffectAPI for RankPeersForChurnEffect {
    type Response = Vec<crate::performance::ChurnRank>;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        self.wrap(|this| async move {
            let inputs =
                enqueue_query(&perf, |reply| PerformanceOp::Peer(PeerOp::RankPeersForChurn { effect: this, reply }))
                    .await;
            rank_churn(inputs)
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ScoresEffect {
    pub(crate) peer: Peer,
}

impl ExternalEffectAPI for ScoresEffect {
    type Response = PeerScores;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        self.wrap(|this| async move {
            enqueue_query(&perf, |reply| PerformanceOp::Peer(PeerOp::Scores { effect: this, reply })).await
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SetLedgerCandidatesEffect {
    pub(crate) candidates: std::collections::BTreeSet<PeerCandidate>,
}

impl ExternalEffectAPI for SetLedgerCandidatesEffect {
    type Response = ();

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let perf = require_perf(&resources);
            enqueue(&perf, PerformanceOp::Peer(PeerOp::SetLedgerCandidates { effect: self.as_ref().clone() }));
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SelectOutboundEffect {
    pub(crate) params: SelectOutboundParams,
}

impl ExternalEffectAPI for SelectOutboundEffect {
    type Response = SelectUsing;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        self.wrap(|this| async move {
            let excluded = this.params.excluded.clone();
            let inputs =
                enqueue_query(&perf, |reply| PerformanceOp::Peer(PeerOp::OutboundInputs { excluded, reply })).await;
            select_outbound_from(&inputs, &this.params)
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct QueryPeerViewEffect {
    pub(crate) since_generation: u64,
}

impl ExternalEffectAPI for QueryPeerViewEffect {
    type Response = Option<PeerView>;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        self.wrap(|this| async move {
            enqueue_query(&perf, |reply| {
                PerformanceOp::Peer(PeerOp::QueryPeerView { since_generation: this.since_generation, reply })
            })
            .await
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct IsStaticPeerEffect {
    pub(crate) peer: Peer,
}

impl ExternalEffectAPI for IsStaticPeerEffect {
    type Response = bool;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        self.wrap(|this| async move {
            enqueue_query(&perf, |reply| PerformanceOp::Peer(PeerOp::IsStaticPeer { effect: this, reply })).await
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NoteDialEffect {
    pub(crate) origin: crate::performance::PeerSource,
    pub(crate) candidate: PeerCandidate,
    pub(crate) peer: Peer,
    pub(crate) at: Instant,
}

impl ExternalEffectAPI for NoteDialEffect {
    type Response = ();

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let perf = require_perf(&resources);
            enqueue(&perf, PerformanceOp::Peer(PeerOp::NoteDial { effect: self.as_ref().clone() }));
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SharedContainsEffect {
    pub(crate) peer: Peer,
}

impl ExternalEffectAPI for SharedContainsEffect {
    type Response = bool;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        self.wrap(|this| async move {
            enqueue_query(&perf, |reply| PerformanceOp::Peer(PeerOp::SharedContains { effect: this, reply })).await
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SourceCountsEffect;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EvictRecordsEffect {
    pub(crate) now: Instant,
    pub(crate) protected_peers: BTreeSet<Peer>,
    pub(crate) protected_candidates: BTreeSet<PeerCandidate>,
}

impl ExternalEffectAPI for EvictRecordsEffect {
    type Response = ();

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let perf = require_perf(&resources);
            enqueue(&perf, PerformanceOp::Peer(PeerOp::EvictRecords { effect: self.as_ref().clone() }));
        })
    }
}

impl ExternalEffectAPI for SourceCountsEffect {
    type Response = crate::performance::SourceCounts;

    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        self.wrap(|this| async move {
            enqueue_query(&perf, |reply| PerformanceOp::Peer(PeerOp::SourceCounts { effect: this, reply })).await
        })
    }
}
