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
//! Each effect is enqueued as a [`PerformanceOp`] on the worker thread.
//!
//! Header/fork telemetry is produced on the worker as pure [`HeaderTelemetry`] values and
//! **emitted here** on the pure-stage effect executor. That keeps OpenTelemetry export (which may
//! drop or lag under resource/connectivity pressure) off the performance worker.

// wrap_sync type safety requires this -- clippy be damned
#![expect(clippy::unit_arg)]

use std::{sync::Arc, time::Duration};

use amaru_kernel::{BlockHeight, HeaderHash, Peer, Tip};
use amaru_protocols::metrics_effects::ResourceMeter;
use amaru_pure_stage::{BoxFuture, ExternalEffect, ExternalEffectAPI, Instant, Resources, SendData};
use tokio::sync::oneshot;

use super::{
    ClaimKind, FetchPeerSet, HeaderLifecycleOutcome, HeaderPerformance, HeaderTelemetry, OutboundCandidateWeight,
    PeerScores, PeerShareFlags, PeerSnapshot, Performance, PerformanceOp, ResourcePerformance, SelectPeersParams,
};

fn require_perf(resources: &Resources) -> ResourcePerformance {
    #[expect(clippy::expect_used)]
    resources.get::<ResourcePerformance>().expect("Performance effect requires ResourcePerformance").clone()
}

fn optional_meter(resources: &Resources) -> Option<Arc<amaru_metrics::Meter>> {
    resources.get::<ResourceMeter>().ok().map(|m| m.clone())
}

/// Fire-and-forget: enqueue and return immediately (does not wait for the worker).
fn enqueue(perf: &Performance, op: PerformanceOp) {
    perf.submit(op);
}

/// Enqueue a query and await the oneshot reply without blocking a multi-thread Tokio worker.
async fn enqueue_query<T: Send + 'static>(
    perf: &Performance,
    make: impl FnOnce(oneshot::Sender<T>) -> PerformanceOp,
) -> T {
    let (reply_tx, reply_rx) = oneshot::channel();
    perf.submit(make(reply_tx));
    #[expect(clippy::expect_used)]
    {
        reply_rx.await.expect("performance worker dropped reply")
    }
}

/// Await worker telemetry, then emit on this (effect-executor) path.
async fn enqueue_and_emit_telemetry(
    perf: &Performance,
    meter: Option<Arc<amaru_metrics::Meter>>,
    make: impl FnOnce(oneshot::Sender<Vec<HeaderTelemetry>>) -> PerformanceOp,
) {
    let events = enqueue_query(perf, make).await;
    HeaderTelemetry::emit_all(&events, meter.as_deref());
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
        slot_start_to_header_micros: u64,
    ) -> RecordHeaderAnnouncementEffect {
        RecordHeaderAnnouncementEffect { peer, header, parent, at, slot_start_to_header_micros }
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

    /// Record latest handshake peer-sharing willingness (`peer_sharing == 1` ⇒ advertisable).
    pub fn record_advertisability(peer: Peer, advertisable: bool, at: Instant) -> RecordAdvertisabilityEffect {
        RecordAdvertisabilityEffect { peer, advertisable, at }
    }

    /// Record a connection/protocol failure (increments `failure_count`).
    pub fn record_connection_failure(peer: Peer, at: Instant) -> RecordConnectionFailureEffect {
        RecordConnectionFailureEffect { peer, at }
    }

    pub fn clear_peer_availability(peer: Peer) -> ClearPeerAvailabilityEffect {
        ClearPeerAvailabilityEffect { peer }
    }

    /// Mark a peer as adversarial: clear claims/scores, keep a reputation stub with `adversarial = true`,
    /// and apply an adversarial malus impulse at `at`.
    pub fn peer_adversarial(peer: Peer, at: Instant) -> PeerAdversarialEffect {
        PeerAdversarialEffect { peer, at }
    }

    pub fn prune_below(min_height: BlockHeight, now: Instant) -> PruneBelowEffect {
        PruneBelowEffect { min_height, now }
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

    pub fn share_flags(peer: Peer) -> ShareFlagsEffect {
        ShareFlagsEffect { peer }
    }

    pub fn snapshot(peer: Peer) -> SnapshotEffect {
        SnapshotEffect { peer }
    }

    pub fn ok_for_sharing(peer: Peer, now: Instant) -> OkForSharingEffect {
        OkForSharingEffect { peer, now }
    }

    /// Sampling weights for outbound peer selection (malus at `half_life`, goodness from scores).
    pub fn outbound_weights(
        candidates: Vec<Peer>,
        half_life: std::time::Duration,
        now: Instant,
    ) -> OutboundWeightsEffect {
        OutboundWeightsEffect { candidates, half_life, now }
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

    pub fn record_block_valid(hash: HeaderHash, now: Instant, syncing: bool) -> RecordBlockValidEffect {
        RecordBlockValidEffect { hash, now, syncing }
    }

    pub fn record_block_pruned(
        hash: HeaderHash,
        invalid: bool,
        now: Instant,
        syncing: bool,
    ) -> RecordBlockPrunedEffect {
        RecordBlockPrunedEffect { hash, invalid, now, syncing }
    }
}

// ---------------------------------------------------------------------------
// Effect types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordIntersectionEffect {
    pub(crate) peer: Peer,
    pub(crate) current: Tip,
    pub(crate) parent: Option<HeaderHash>,
    pub(crate) at: Instant,
}

impl ExternalEffect for RecordIntersectionEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let perf = require_perf(&resources);
            enqueue(&perf, PerformanceOp::RecordIntersection { effect: *self });
        })
    }
}

impl ExternalEffectAPI for RecordIntersectionEffect {
    type Response = ();
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordHeaderAnnouncementEffect {
    pub(crate) peer: Peer,
    pub(crate) header: Tip,
    pub(crate) parent: Option<HeaderHash>,
    pub(crate) at: Instant,
    /// Stage-computed interval from virtual slot start to header reception.
    pub(crate) slot_start_to_header_micros: u64,
}

impl ExternalEffect for RecordHeaderAnnouncementEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let perf = require_perf(&resources);
            enqueue(&perf, PerformanceOp::RecordHeaderAnnouncement { effect: *self });
        })
    }
}

impl ExternalEffectAPI for RecordHeaderAnnouncementEffect {
    type Response = ();
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordBlocksRequestedEffect {
    pub(crate) hashes: Vec<HeaderHash>,
    pub(crate) requested_at: Instant,
}

impl ExternalEffect for RecordBlocksRequestedEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let perf = require_perf(&resources);
            enqueue(&perf, PerformanceOp::RecordBlocksRequested { effect: *self });
        })
    }
}

impl ExternalEffectAPI for RecordBlocksRequestedEffect {
    type Response = ();
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordBlockDeliveryEffect {
    pub(crate) peer: Peer,
    pub(crate) hash: HeaderHash,
    pub(crate) height: BlockHeight,
    pub(crate) parent: Option<HeaderHash>,
    pub(crate) at: Instant,
    pub(crate) response: Duration,
    pub(crate) bytes: u64,
}

impl ExternalEffect for RecordBlockDeliveryEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let perf = require_perf(&resources);
            enqueue(&perf, PerformanceOp::RecordBlockDelivery { effect: *self });
        })
    }
}

impl ExternalEffectAPI for RecordBlockDeliveryEffect {
    type Response = ();
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordFetchFailureEffect {
    pub(crate) peers: Vec<Peer>,
    pub(crate) at: Instant,
}

impl ExternalEffect for RecordFetchFailureEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let perf = require_perf(&resources);
            enqueue(&perf, PerformanceOp::RecordFetchFailure { effect: *self });
        })
    }
}

impl ExternalEffectAPI for RecordFetchFailureEffect {
    type Response = ();
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordKeepaliveRttEffect {
    pub(crate) peer: Peer,
    pub(crate) rtt: Duration,
    pub(crate) at: Instant,
}

impl ExternalEffect for RecordKeepaliveRttEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let perf = require_perf(&resources);
            enqueue(&perf, PerformanceOp::RecordKeepaliveRtt { effect: *self });
        })
    }
}

impl ExternalEffectAPI for RecordKeepaliveRttEffect {
    type Response = ();
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordAdvertisabilityEffect {
    pub(crate) peer: Peer,
    pub(crate) advertisable: bool,
    pub(crate) at: Instant,
}

impl ExternalEffect for RecordAdvertisabilityEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let perf = require_perf(&resources);
            enqueue(&perf, PerformanceOp::RecordAdvertisability { effect: *self });
        })
    }
}

impl ExternalEffectAPI for RecordAdvertisabilityEffect {
    type Response = ();
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordConnectionFailureEffect {
    pub(crate) peer: Peer,
    pub(crate) at: Instant,
}

impl ExternalEffect for RecordConnectionFailureEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let perf = require_perf(&resources);
            enqueue(&perf, PerformanceOp::RecordConnectionFailure { effect: *self });
        })
    }
}

impl ExternalEffectAPI for RecordConnectionFailureEffect {
    type Response = ();
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ClearPeerAvailabilityEffect {
    pub(crate) peer: Peer,
}

impl ExternalEffect for ClearPeerAvailabilityEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let perf = require_perf(&resources);
            enqueue(&perf, PerformanceOp::ClearPeerAvailability { effect: *self });
        })
    }
}

impl ExternalEffectAPI for ClearPeerAvailabilityEffect {
    type Response = ();
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PeerAdversarialEffect {
    pub(crate) peer: Peer,
    pub(crate) at: Instant,
}

impl ExternalEffect for PeerAdversarialEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let perf = require_perf(&resources);
            enqueue(&perf, PerformanceOp::PeerAdversarial { effect: *self });
        })
    }
}

impl ExternalEffectAPI for PeerAdversarialEffect {
    type Response = ();
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PruneBelowEffect {
    pub(crate) min_height: BlockHeight,
    pub(crate) now: Instant,
}

impl ExternalEffect for PruneBelowEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        let meter = optional_meter(&resources);
        Self::wrap(async move {
            enqueue_and_emit_telemetry(&perf, meter, |reply| PerformanceOp::PruneBelow { effect: *self, reply }).await
        })
    }
}

impl ExternalEffectAPI for PruneBelowEffect {
    type Response = ();
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SelectPeersForFetchEffect {
    pub(crate) params: SelectPeersParams,
}

impl ExternalEffect for SelectPeersForFetchEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        Self::wrap(async move {
            enqueue_query(&perf, |reply| PerformanceOp::SelectPeersForFetch { effect: *self, reply }).await
        })
    }
}

impl ExternalEffectAPI for SelectPeersForFetchEffect {
    type Response = FetchPeerSet;
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PeerCoversFragmentEffect {
    pub(crate) peer: Peer,
    pub(crate) need: Vec<HeaderHash>,
}

impl ExternalEffect for PeerCoversFragmentEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        Self::wrap(async move {
            enqueue_query(&perf, |reply| PerformanceOp::PeerCoversFragment { effect: *self, reply }).await
        })
    }
}

impl ExternalEffectAPI for PeerCoversFragmentEffect {
    type Response = bool;
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DirectClaimantsEffect {
    pub(crate) hash: HeaderHash,
}

impl ExternalEffect for DirectClaimantsEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        Self::wrap(async move {
            enqueue_query(&perf, |reply| PerformanceOp::DirectClaimants { effect: *self, reply }).await
        })
    }
}

impl ExternalEffectAPI for DirectClaimantsEffect {
    type Response = Vec<(Peer, Instant, ClaimKind)>;
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FirstAnnouncedAtEffect {
    pub(crate) hash: HeaderHash,
}

impl ExternalEffect for FirstAnnouncedAtEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        Self::wrap(async move {
            enqueue_query(&perf, |reply| PerformanceOp::FirstAnnouncedAt { effect: *self, reply }).await
        })
    }
}

impl ExternalEffectAPI for FirstAnnouncedAtEffect {
    type Response = Option<(Peer, Instant)>;
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RankPeersForChurnEffect {
    pub(crate) candidates: Vec<Peer>,
    pub(crate) now: Instant,
}

impl ExternalEffect for RankPeersForChurnEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        Self::wrap(async move {
            enqueue_query(&perf, |reply| PerformanceOp::RankPeersForChurn { effect: *self, reply }).await
        })
    }
}

impl ExternalEffectAPI for RankPeersForChurnEffect {
    type Response = Vec<(Peer, PeerScores)>;
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ScoresEffect {
    pub(crate) peer: Peer,
}

impl ExternalEffect for ScoresEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        Self::wrap(async move { enqueue_query(&perf, |reply| PerformanceOp::Scores { effect: *self, reply }).await })
    }
}

impl ExternalEffectAPI for ScoresEffect {
    type Response = PeerScores;
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ShareFlagsEffect {
    pub(crate) peer: Peer,
}

impl ExternalEffect for ShareFlagsEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        Self::wrap(
            async move { enqueue_query(&perf, |reply| PerformanceOp::ShareFlags { effect: *self, reply }).await },
        )
    }
}

impl ExternalEffectAPI for ShareFlagsEffect {
    type Response = Option<PeerShareFlags>;
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SnapshotEffect {
    pub(crate) peer: Peer,
}

impl ExternalEffect for SnapshotEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        Self::wrap(async move { enqueue_query(&perf, |reply| PerformanceOp::Snapshot { effect: *self, reply }).await })
    }
}

impl ExternalEffectAPI for SnapshotEffect {
    type Response = Option<PeerSnapshot>;
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OkForSharingEffect {
    pub(crate) peer: Peer,
    pub(crate) now: Instant,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OutboundWeightsEffect {
    pub(crate) candidates: Vec<Peer>,
    pub(crate) half_life: std::time::Duration,
    pub(crate) now: Instant,
}

impl ExternalEffect for OutboundWeightsEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        Self::wrap(async move {
            enqueue_query(&perf, |reply| PerformanceOp::OutboundWeights { effect: *self, reply }).await
        })
    }
}

impl ExternalEffectAPI for OutboundWeightsEffect {
    type Response = Vec<OutboundCandidateWeight>;
}

impl ExternalEffect for OkForSharingEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        Self::wrap(
            async move { enqueue_query(&perf, |reply| PerformanceOp::OkForSharing { effect: *self, reply }).await },
        )
    }
}

impl ExternalEffectAPI for OkForSharingEffect {
    type Response = bool;
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordRollbackEffect {
    pub(crate) peer: Peer,
    pub(crate) point: Tip,
    pub(crate) parent: Option<HeaderHash>,
    pub(crate) at: Instant,
}

impl ExternalEffect for RecordRollbackEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let perf = require_perf(&resources);
            enqueue(&perf, PerformanceOp::RecordRollback { effect: *self });
        })
    }
}

impl ExternalEffectAPI for RecordRollbackEffect {
    type Response = ();
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordHeaderRejectedEffect {
    pub(crate) outcome: HeaderLifecycleOutcome,
}

impl ExternalEffect for RecordHeaderRejectedEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            // NOTE: No worker state; emit directly on the effect path (never on the performance thread).
            let meter = optional_meter(&resources);
            HeaderPerformance::apply_header_rejected(self.outcome).emit(meter.as_deref());
        })
    }
}

impl ExternalEffectAPI for RecordHeaderRejectedEffect {
    type Response = ();
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordHeaderAbandonedEffect {
    pub(crate) hash: HeaderHash,
    pub(crate) now: Instant,
}

impl ExternalEffect for RecordHeaderAbandonedEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        let meter = optional_meter(&resources);
        Self::wrap(async move {
            enqueue_and_emit_telemetry(&perf, meter, |reply| PerformanceOp::RecordHeaderAbandoned {
                effect: *self,
                reply,
            })
            .await
        })
    }
}

impl ExternalEffectAPI for RecordHeaderAbandonedEffect {
    type Response = ();
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordForkStartedEffect {
    pub(crate) tip: Tip,
    pub(crate) started_at: Instant,
}

impl ExternalEffect for RecordForkStartedEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        let meter = optional_meter(&resources);
        Self::wrap(async move {
            enqueue_and_emit_telemetry(&perf, meter, |reply| PerformanceOp::RecordForkStarted { effect: *self, reply })
                .await
        })
    }
}

impl ExternalEffectAPI for RecordForkStartedEffect {
    type Response = ();
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordBlockValidEffect {
    pub(crate) hash: HeaderHash,
    pub(crate) now: Instant,
    /// When true, omit `slot_start_to_header_micros` from the emitted lifecycle metric.
    pub(crate) syncing: bool,
}

impl ExternalEffect for RecordBlockValidEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        let meter = optional_meter(&resources);
        Self::wrap(async move {
            enqueue_and_emit_telemetry(&perf, meter, |reply| PerformanceOp::RecordBlockValid { effect: *self, reply })
                .await
        })
    }
}

impl ExternalEffectAPI for RecordBlockValidEffect {
    type Response = ();
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordBlockPrunedEffect {
    pub(crate) hash: HeaderHash,
    pub(crate) invalid: bool,
    pub(crate) now: Instant,
    /// When true, omit `slot_start_to_header_micros` from the emitted lifecycle metric.
    pub(crate) syncing: bool,
}

impl ExternalEffect for RecordBlockPrunedEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let perf = require_perf(&resources);
        let meter = optional_meter(&resources);
        Self::wrap(async move {
            enqueue_and_emit_telemetry(&perf, meter, |reply| PerformanceOp::RecordBlockPruned { effect: *self, reply })
                .await
        })
    }
}

impl ExternalEffectAPI for RecordBlockPrunedEffect {
    type Response = ();
}
