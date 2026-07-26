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

//! External-effect constructors and implementations for [`Performance`] events.
//!
//! Stages use factory methods on [`Performance`] and pass the result to `eff.external(...)`.

use std::{sync::Arc, time::Duration};

use amaru_kernel::{BlockHeight, HeaderHash, Peer, Tip};
use amaru_metrics::Meter;
use amaru_protocols::metrics_effects::ResourceMeter;
use amaru_pure_stage::{BoxFuture, ExternalEffect, ExternalEffectAPI, Instant, Resources, SendData};

use super::{
    ClaimKind, FetchPeerSet, HeaderLifecycleOutcome, PeerScores, PeerSnapshot, Performance, ResourcePerformance,
    SelectPeersParams,
};

fn require_perf(resources: &Resources) -> ResourcePerformance {
    #[expect(clippy::expect_used)]
    resources.get::<ResourcePerformance>().expect("Performance effect requires ResourcePerformance").clone()
}

fn optional_meter(resources: &Resources) -> Option<Arc<Meter>> {
    resources.get::<ResourceMeter>().ok().map(|m| m.clone())
}

// ---------------------------------------------------------------------------
// Static constructors on Performance (stage-facing event API)
// ---------------------------------------------------------------------------

impl Performance {
    pub fn record_intersection(
        peer: Peer,
        current: Tip,
        parent: Option<HeaderHash>,
        at: Instant,
    ) -> RecordIntersectionEffect {
        RecordIntersectionEffect { peer, current, parent, at }
    }

    pub fn record_header_announcement(
        peer: Peer,
        header: Tip,
        parent: Option<HeaderHash>,
        at: Instant,
    ) -> RecordHeaderAnnouncementEffect {
        RecordHeaderAnnouncementEffect { peer, header, parent, at }
    }

    pub fn record_blocks_requested(hashes: Vec<HeaderHash>, requested_at: Instant) -> RecordBlocksRequestedEffect {
        RecordBlocksRequestedEffect { hashes, requested_at }
    }

    pub fn record_block_delivery(
        peer: Peer,
        hash: HeaderHash,
        height: BlockHeight,
        parent: Option<HeaderHash>,
        at: Instant,
        response: Duration,
        bytes: u64,
    ) -> RecordBlockDeliveryEffect {
        RecordBlockDeliveryEffect { peer, hash, height, parent, at, response, bytes }
    }

    pub fn record_fetch_failure(peers: Vec<Peer>, at: Instant) -> RecordFetchFailureEffect {
        RecordFetchFailureEffect { peers, at }
    }

    pub fn record_keepalive_rtt(peer: Peer, rtt: Duration, at: Instant) -> RecordKeepaliveRttEffect {
        RecordKeepaliveRttEffect { peer, rtt, at }
    }

    pub fn clear_peer_availability(peer: Peer) -> ClearPeerAvailabilityEffect {
        ClearPeerAvailabilityEffect { peer }
    }

    pub fn forget_peer(peer: Peer) -> ForgetPeerEffect {
        ForgetPeerEffect { peer }
    }

    pub fn prune_below(min_height: BlockHeight) -> PruneBelowEffect {
        PruneBelowEffect { min_height }
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

    pub fn rank_peers_for_churn(candidates: Vec<Peer>, now: Instant) -> RankPeersForChurnEffect {
        RankPeersForChurnEffect { candidates, now }
    }

    pub fn scores(peer: Peer) -> ScoresEffect {
        ScoresEffect { peer }
    }

    pub fn snapshot(peer: Peer) -> SnapshotEffect {
        SnapshotEffect { peer }
    }

    pub fn record_rollback(peer: Peer, point: Tip, parent: Option<HeaderHash>, at: Instant) -> RecordRollbackEffect {
        RecordRollbackEffect { peer, point, parent, at }
    }

    pub fn record_header_rejected(outcome: HeaderLifecycleOutcome) -> RecordHeaderRejectedEffect {
        RecordHeaderRejectedEffect { outcome }
    }

    pub fn record_header_abandoned(hash: HeaderHash, now: Instant) -> RecordHeaderAbandonedEffect {
        RecordHeaderAbandonedEffect { hash, now }
    }

    pub fn record_fork_started(tip: Tip, started_at: Instant) -> RecordForkStartedEffect {
        RecordForkStartedEffect { tip, started_at }
    }

    pub fn record_block_valid(hash: HeaderHash, now: Instant) -> RecordBlockValidEffect {
        RecordBlockValidEffect { hash, now }
    }

    pub fn record_block_pruned(hash: HeaderHash, invalid: bool, now: Instant) -> RecordBlockPrunedEffect {
        RecordBlockPrunedEffect { hash, invalid, now }
    }
}

// ---------------------------------------------------------------------------
// Effect types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordIntersectionEffect {
    peer: Peer,
    current: Tip,
    parent: Option<HeaderHash>,
    at: Instant,
}

impl ExternalEffect for RecordIntersectionEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        perf.apply_intersection(self.peer, self.current, self.parent, self.at);
        Self::wrap_sync(())
    }
}

impl ExternalEffectAPI for RecordIntersectionEffect {
    type Response = ();
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordHeaderAnnouncementEffect {
    peer: Peer,
    header: Tip,
    parent: Option<HeaderHash>,
    at: Instant,
}

impl ExternalEffect for RecordHeaderAnnouncementEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        perf.apply_header_announcement(self.peer, self.header, self.parent, self.at);
        Self::wrap_sync(())
    }
}

impl ExternalEffectAPI for RecordHeaderAnnouncementEffect {
    type Response = ();
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordBlocksRequestedEffect {
    hashes: Vec<HeaderHash>,
    requested_at: Instant,
}

impl ExternalEffect for RecordBlocksRequestedEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        perf.apply_blocks_requested(&self.hashes, self.requested_at);
        Self::wrap_sync(())
    }
}

impl ExternalEffectAPI for RecordBlocksRequestedEffect {
    type Response = ();
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordBlockDeliveryEffect {
    peer: Peer,
    hash: HeaderHash,
    height: BlockHeight,
    parent: Option<HeaderHash>,
    at: Instant,
    response: Duration,
    bytes: u64,
}

impl ExternalEffect for RecordBlockDeliveryEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        perf.apply_block_delivery(self.peer, self.hash, self.height, self.parent, self.at, self.response, self.bytes);
        Self::wrap_sync(())
    }
}

impl ExternalEffectAPI for RecordBlockDeliveryEffect {
    type Response = ();
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordFetchFailureEffect {
    peers: Vec<Peer>,
    at: Instant,
}

impl ExternalEffect for RecordFetchFailureEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        perf.apply_fetch_failure(&self.peers, self.at);
        Self::wrap_sync(())
    }
}

impl ExternalEffectAPI for RecordFetchFailureEffect {
    type Response = ();
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordKeepaliveRttEffect {
    peer: Peer,
    rtt: Duration,
    at: Instant,
}

impl ExternalEffect for RecordKeepaliveRttEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        perf.apply_keepalive_rtt(self.peer, self.rtt, self.at);
        Self::wrap_sync(())
    }
}

impl ExternalEffectAPI for RecordKeepaliveRttEffect {
    type Response = ();
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ClearPeerAvailabilityEffect {
    peer: Peer,
}

impl ExternalEffect for ClearPeerAvailabilityEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        perf.apply_clear_peer_availability(&self.peer);
        Self::wrap_sync(())
    }
}

impl ExternalEffectAPI for ClearPeerAvailabilityEffect {
    type Response = ();
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ForgetPeerEffect {
    peer: Peer,
}

impl ExternalEffect for ForgetPeerEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        perf.apply_forget_peer(&self.peer);
        Self::wrap_sync(())
    }
}

impl ExternalEffectAPI for ForgetPeerEffect {
    type Response = ();
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PruneBelowEffect {
    min_height: BlockHeight,
}

impl ExternalEffect for PruneBelowEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        perf.apply_prune_below(self.min_height);
        Self::wrap_sync(())
    }
}

impl ExternalEffectAPI for PruneBelowEffect {
    type Response = ();
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SelectPeersForFetchEffect {
    params: SelectPeersParams,
}

impl ExternalEffect for SelectPeersForFetchEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        let result = perf.apply_select_peers_for_fetch(self.params);
        Self::wrap_sync(result)
    }
}

impl ExternalEffectAPI for SelectPeersForFetchEffect {
    type Response = FetchPeerSet;
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PeerCoversFragmentEffect {
    peer: Peer,
    need: Vec<HeaderHash>,
}

impl ExternalEffect for PeerCoversFragmentEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        let result = perf.apply_peer_covers_fragment(&self.peer, &self.need);
        Self::wrap_sync(result)
    }
}

impl ExternalEffectAPI for PeerCoversFragmentEffect {
    type Response = bool;
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DirectClaimantsEffect {
    hash: HeaderHash,
}

impl ExternalEffect for DirectClaimantsEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        let result = perf.apply_direct_claimants(&self.hash);
        Self::wrap_sync(result)
    }
}

impl ExternalEffectAPI for DirectClaimantsEffect {
    type Response = Vec<(Peer, Instant, ClaimKind)>;
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FirstAnnouncedAtEffect {
    hash: HeaderHash,
}

impl ExternalEffect for FirstAnnouncedAtEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        let result = perf.apply_first_announced_at(&self.hash);
        Self::wrap_sync(result)
    }
}

impl ExternalEffectAPI for FirstAnnouncedAtEffect {
    type Response = Option<(Peer, Instant)>;
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RankPeersForChurnEffect {
    candidates: Vec<Peer>,
    now: Instant,
}

impl ExternalEffect for RankPeersForChurnEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        let result = perf.apply_rank_peers_for_churn(&self.candidates, self.now);
        Self::wrap_sync(result)
    }
}

impl ExternalEffectAPI for RankPeersForChurnEffect {
    type Response = Vec<(Peer, PeerScores)>;
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ScoresEffect {
    peer: Peer,
}

impl ExternalEffect for ScoresEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        let result = perf.apply_scores(&self.peer);
        Self::wrap_sync(result)
    }
}

impl ExternalEffectAPI for ScoresEffect {
    type Response = PeerScores;
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SnapshotEffect {
    peer: Peer,
}

impl ExternalEffect for SnapshotEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        let result = perf.apply_snapshot(&self.peer);
        Self::wrap_sync(result)
    }
}

impl ExternalEffectAPI for SnapshotEffect {
    type Response = Option<PeerSnapshot>;
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordRollbackEffect {
    peer: Peer,
    point: Tip,
    parent: Option<HeaderHash>,
    at: Instant,
}

impl ExternalEffect for RecordRollbackEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        perf.apply_rollback(self.peer, self.point, self.parent, self.at);
        Self::wrap_sync(())
    }
}

impl ExternalEffectAPI for RecordRollbackEffect {
    type Response = ();
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordHeaderRejectedEffect {
    outcome: HeaderLifecycleOutcome,
}

impl ExternalEffect for RecordHeaderRejectedEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        let meter = optional_meter(&resources);
        perf.apply_header_rejected(self.outcome, meter.as_deref());
        Self::wrap_sync(())
    }
}

impl ExternalEffectAPI for RecordHeaderRejectedEffect {
    type Response = ();
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordHeaderAbandonedEffect {
    hash: HeaderHash,
    now: Instant,
}

impl ExternalEffect for RecordHeaderAbandonedEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        let meter = optional_meter(&resources);
        perf.apply_header_abandoned(&self.hash, self.now, meter.as_deref());
        Self::wrap_sync(())
    }
}

impl ExternalEffectAPI for RecordHeaderAbandonedEffect {
    type Response = ();
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordForkStartedEffect {
    tip: Tip,
    started_at: Instant,
}

impl ExternalEffect for RecordForkStartedEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        let meter = optional_meter(&resources);
        perf.apply_fork_started(self.tip, self.started_at, meter.as_deref());
        Self::wrap_sync(())
    }
}

impl ExternalEffectAPI for RecordForkStartedEffect {
    type Response = ();
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordBlockValidEffect {
    hash: HeaderHash,
    now: Instant,
}

impl ExternalEffect for RecordBlockValidEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        let meter = optional_meter(&resources);
        perf.apply_block_valid(&self.hash, self.now, meter.as_deref());
        Self::wrap_sync(())
    }
}

impl ExternalEffectAPI for RecordBlockValidEffect {
    type Response = ();
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordBlockPrunedEffect {
    hash: HeaderHash,
    invalid: bool,
    now: Instant,
}

impl ExternalEffect for RecordBlockPrunedEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        let meter = optional_meter(&resources);
        perf.apply_block_pruned(&self.hash, self.invalid, self.now, meter.as_deref());
        Self::wrap_sync(())
    }
}

impl ExternalEffectAPI for RecordBlockPrunedEffect {
    type Response = ();
}
