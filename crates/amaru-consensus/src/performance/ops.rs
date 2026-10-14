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

//! Operations the performance worker runs, grouped by the state they touch.
//!
//! A header announcement or a block delivery updates peer claims and header lifecycle together,
//! so those ops sit with the peer group and the header update stays in the same arm. Prune
//! updates both maps and sits with the header group. Pace is only the sync-adoption window.

use std::{collections::BTreeSet, net::SocketAddr, time::Duration};

use amaru_kernel::{Peer, PeerCandidate};
use amaru_ouroboros::{CloseReason, ConnectionId, ConnectionRecord, MiniProtocolUse, ObservedAt, RemoteProtocol};
use amaru_pure_stage::Instant;
use tokio::sync::oneshot;

use super::{
    ChurnInput, ClaimKind, FetchPeerSet, OutboundInputs, PeerScores, PeerShareFlags, PeerSnapshot, PeerView,
    SharedIngestResult, SourceCounts,
    adoption::SyncAdoptionPace,
    effects::{
        ClearPeerAvailabilityEffect, DirectClaimantsEffect, EvictRecordsEffect, FirstAnnouncedAtEffect,
        IsStaticPeerEffect, NoteDialEffect, OkForSharingEffect, PeerAdversarialEffect, PeerCoversFragmentEffect,
        PruneBelowEffect, RankPeersForChurnEffect, RecordAdvertisabilityEffect, RecordBlockDeliveryEffect,
        RecordBlockPrunedEffect, RecordBlockValidEffect, RecordBlocksRequestedEffect, RecordConnectionFailureEffect,
        RecordFetchFailureEffect, RecordForkStartedEffect, RecordHeaderAbandonedEffect, RecordHeaderAnnouncementEffect,
        RecordIntersectionEffect, RecordKeepaliveRttEffect, RecordPeersAskedEffect, RecordRollbackEffect,
        RecordSyncAdoptionEffect, ScoresEffect, SelectPeersForFetchEffect, SetLedgerCandidatesEffect, ShareFlagsEffect,
        SharedContainsEffect, SnapshotEffect, SourceCountsEffect, SyncAdoptionPaceEffect,
    },
    header::{HeaderPerformance, HeaderTelemetry},
    peers::{PeerPerformance, ShareCandidate},
};

#[cfg(test)]
pub(crate) fn is_urgent_op(op: &PerformanceOp) -> bool {
    matches!(op, PerformanceOp::Peer(PeerOp::RecordHeaderAnnouncement { .. } | PeerOp::SelectPeersForFetch { .. }))
}

pub(crate) enum PerformanceOp {
    Peer(PeerOp),
    Header(HeaderOp),
    Pace(PaceOp),
}

pub(crate) enum PeerOp {
    RecordIntersection {
        effect: RecordIntersectionEffect,
    },
    RecordHeaderAnnouncement {
        effect: RecordHeaderAnnouncementEffect,
        reply: oneshot::Sender<Vec<HeaderTelemetry>>,
    },
    RecordBlockDelivery {
        effect: RecordBlockDeliveryEffect,
        reply: oneshot::Sender<Vec<HeaderTelemetry>>,
    },
    RecordFetchFailure {
        effect: RecordFetchFailureEffect,
    },
    RecordKeepaliveRtt {
        effect: RecordKeepaliveRttEffect,
    },
    RecordAdvertisability {
        effect: RecordAdvertisabilityEffect,
    },
    RecordConnectionFailure {
        effect: RecordConnectionFailureEffect,
    },
    ClearPeerAvailability {
        effect: ClearPeerAvailabilityEffect,
    },
    PeerAdversarial {
        effect: PeerAdversarialEffect,
    },
    SelectPeersForFetch {
        effect: SelectPeersForFetchEffect,
        reply: oneshot::Sender<FetchPeerSet>,
    },
    PeerCoversFragment {
        effect: PeerCoversFragmentEffect,
        reply: oneshot::Sender<bool>,
    },
    DirectClaimants {
        effect: DirectClaimantsEffect,
        reply: oneshot::Sender<Vec<(Peer, Instant, ClaimKind)>>,
    },
    FirstAnnouncedAt {
        effect: FirstAnnouncedAtEffect,
        reply: oneshot::Sender<Option<(Peer, Instant)>>,
    },
    RankPeersForChurn {
        effect: RankPeersForChurnEffect,
        reply: oneshot::Sender<Vec<ChurnInput>>,
    },
    Scores {
        effect: ScoresEffect,
        reply: oneshot::Sender<PeerScores>,
    },
    ShareFlags {
        effect: ShareFlagsEffect,
        reply: oneshot::Sender<Option<PeerShareFlags>>,
    },
    Snapshot {
        effect: SnapshotEffect,
        reply: oneshot::Sender<Option<PeerSnapshot>>,
    },
    OkForSharing {
        effect: OkForSharingEffect,
        reply: oneshot::Sender<bool>,
    },
    SetLedgerCandidates {
        effect: SetLedgerCandidatesEffect,
    },
    EvictRecords {
        effect: EvictRecordsEffect,
    },
    OutboundInputs {
        excluded: BTreeSet<PeerCandidate>,
        reply: oneshot::Sender<OutboundInputs>,
    },
    QueryPeerView {
        since_generation: u64,
        reply: oneshot::Sender<Option<PeerView>>,
    },
    ShareReplyCandidates {
        now: Instant,
        reply: oneshot::Sender<Vec<ShareCandidate>>,
    },
    IsStaticPeer {
        effect: IsStaticPeerEffect,
        reply: oneshot::Sender<bool>,
    },
    NoteDial {
        effect: NoteDialEffect,
    },
    SharedContains {
        effect: SharedContainsEffect,
        reply: oneshot::Sender<bool>,
    },
    SourceCounts {
        effect: SourceCountsEffect,
        reply: oneshot::Sender<SourceCounts>,
    },
    RecordRollback {
        effect: RecordRollbackEffect,
    },
    RecordConnectionEstablished {
        conn: ConnectionRecord,
        at: ObservedAt,
        reply: oneshot::Sender<()>,
    },
    RecordConnectionClosed {
        peer: Peer,
        conn_id: ConnectionId,
        reason: CloseReason,
        at: ObservedAt,
    },
    RecordConnectFailed {
        peer: Peer,
        at: ObservedAt,
    },
    RecordLocalUseApplied {
        peer: Peer,
        conn_id: ConnectionId,
        local_use: MiniProtocolUse,
        at: ObservedAt,
    },
    MayInitiate {
        reply: oneshot::Sender<BTreeSet<ConnectionId>>,
    },
    RecordRemoteUse {
        peer: Peer,
        conn_id: ConnectionId,
        protocol: RemoteProtocol,
        active: bool,
        at: ObservedAt,
        reply: oneshot::Sender<Option<MiniProtocolUse>>,
    },
    RecordKeepaliveSample {
        peer: Peer,
        rtt: Duration,
        at: ObservedAt,
    },
    RecordSharedPeers {
        from: Peer,
        addrs: Vec<SocketAddr>,
        at: ObservedAt,
        reply: oneshot::Sender<SharedIngestResult>,
    },
    RecordShareRequestServed {
        requester: Peer,
        amount: u8,
        at: ObservedAt,
    },
    RecordUninteresting {
        peer: Peer,
        conn_id: ConnectionId,
        after_rollback: bool,
        at: ObservedAt,
    },
}

pub(crate) enum HeaderOp {
    RecordBlocksRequested { effect: RecordBlocksRequestedEffect },
    RecordPeersAsked { effect: RecordPeersAskedEffect, reply: oneshot::Sender<Vec<HeaderTelemetry>> },
    PruneBelow { effect: PruneBelowEffect, reply: oneshot::Sender<Vec<HeaderTelemetry>> },
    RecordHeaderAbandoned { effect: RecordHeaderAbandonedEffect, reply: oneshot::Sender<Vec<HeaderTelemetry>> },
    RecordForkStarted { effect: RecordForkStartedEffect, reply: oneshot::Sender<Vec<HeaderTelemetry>> },
    RecordBlockValid { effect: RecordBlockValidEffect, reply: oneshot::Sender<Vec<HeaderTelemetry>> },
    RecordBlockPruned { effect: RecordBlockPrunedEffect, reply: oneshot::Sender<Vec<HeaderTelemetry>> },
}

pub(crate) enum PaceOp {
    RecordSyncAdoption { effect: RecordSyncAdoptionEffect },
    SyncAdoptionPace { effect: SyncAdoptionPaceEffect, reply: oneshot::Sender<bool> },
}

pub(crate) fn dispatch(
    peers: &mut PeerPerformance,
    headers: &mut HeaderPerformance,
    pace: &mut SyncAdoptionPace,
    op: PerformanceOp,
) {
    match op {
        PerformanceOp::Peer(op) => dispatch_peer(peers, headers, op),
        PerformanceOp::Header(op) => dispatch_header(peers, headers, op),
        PerformanceOp::Pace(op) => dispatch_pace(pace, op),
    }
}

fn dispatch_peer(peers: &mut PeerPerformance, headers: &mut HeaderPerformance, op: PeerOp) {
    match op {
        PeerOp::RecordIntersection { effect } => {
            peers.record_intersection(effect.peer, effect.current, effect.parent, effect.at);
        }
        PeerOp::RecordHeaderAnnouncement { effect, reply } => {
            peers.record_header_announcement(effect.peer, effect.header, effect.parent, effect.at);
            let telemetry = headers.apply_header_received(
                effect.peer,
                effect.header,
                effect.at,
                effect.slot_start_to_header_micros,
                effect.slot_onset,
                effect.already_stored,
            );
            let _ = reply.send(telemetry);
        }
        PeerOp::RecordBlockDelivery { effect, reply } => {
            peers.record_block_delivery(
                effect.peer,
                effect.hash,
                effect.height,
                effect.parent,
                effect.at,
                effect.response,
                effect.bytes,
            );
            let telemetry = headers.apply_block_downloaded(effect.peer, &effect.hash, effect.height, effect.at);
            let _ = reply.send(telemetry);
        }
        PeerOp::RecordFetchFailure { effect } => {
            peers.record_fetch_failure(&effect.peers, effect.at);
        }
        PeerOp::RecordKeepaliveRtt { effect } => {
            peers.record_keepalive_rtt(effect.peer, effect.rtt, effect.at);
        }
        PeerOp::RecordAdvertisability { effect } => {
            peers.record_advertisability(effect.peer, effect.advertisable, effect.at);
        }
        PeerOp::RecordConnectionFailure { effect } => {
            peers.record_connection_failure(effect.peer, effect.at);
        }
        PeerOp::ClearPeerAvailability { effect } => {
            peers.clear_availability(&effect.peer);
        }
        PeerOp::PeerAdversarial { effect } => {
            peers.mark_adversarial(&effect.peer, effect.at);
        }
        PeerOp::SelectPeersForFetch { effect, reply } => {
            let result = peers.select_peers_for_fetch(effect.params);
            let _ = reply.send(result);
        }
        PeerOp::PeerCoversFragment { effect, reply } => {
            let result = peers.peer_covers_fragment(&effect.peer, &effect.need);
            let _ = reply.send(result);
        }
        PeerOp::DirectClaimants { effect, reply } => {
            let result = peers.direct_claimants(&effect.hash);
            let _ = reply.send(result);
        }
        PeerOp::FirstAnnouncedAt { effect, reply } => {
            let result = peers.first_announced_at(&effect.hash);
            let _ = reply.send(result);
        }
        PeerOp::RankPeersForChurn { effect, reply } => {
            let result = peers.churn_inputs(&effect.candidates);
            let _ = reply.send(result);
        }
        PeerOp::Scores { effect, reply } => {
            let result = peers.scores(&effect.peer);
            let _ = reply.send(result);
        }
        PeerOp::ShareFlags { effect, reply } => {
            let result = peers.share_flags(&effect.peer);
            let _ = reply.send(result);
        }
        PeerOp::Snapshot { effect, reply } => {
            let result = peers.snapshot(&effect.peer);
            let _ = reply.send(result);
        }
        PeerOp::OkForSharing { effect, reply } => {
            let result = peers.ok_for_sharing(&effect.peer, effect.now);
            let _ = reply.send(result);
        }
        PeerOp::SetLedgerCandidates { effect } => {
            peers.set_ledger_candidates(effect.candidates);
        }
        PeerOp::EvictRecords { effect } => {
            peers.evict_batch(effect.now, &effect.protected_peers, &effect.protected_candidates);
        }
        PeerOp::OutboundInputs { excluded, reply } => {
            let result = peers.outbound_inputs(&excluded);
            let _ = reply.send(result);
        }
        PeerOp::QueryPeerView { since_generation, reply } => {
            let result = peers.query_peer_view(since_generation);
            let _ = reply.send(result);
        }
        PeerOp::ShareReplyCandidates { now, reply } => {
            let result = peers.share_reply_candidates(now);
            let _ = reply.send(result);
        }
        PeerOp::IsStaticPeer { effect, reply } => {
            let result = peers.is_static_peer(&effect.peer);
            let _ = reply.send(result);
        }
        PeerOp::NoteDial { effect } => {
            peers.note_dial(effect.origin, &effect.candidate, effect.peer, effect.at);
        }
        PeerOp::SharedContains { effect, reply } => {
            let result = peers.shared_contains(&effect.peer);
            let _ = reply.send(result);
        }
        PeerOp::SourceCounts { effect: SourceCountsEffect, reply } => {
            let result = peers.source_counts();
            let _ = reply.send(result);
        }
        PeerOp::RecordRollback { effect } => {
            peers.record_rollback(effect.peer, effect.point, effect.parent, effect.at);
        }
        PeerOp::RecordConnectionEstablished { conn, at, reply } => {
            peers.record_connection_established(conn, at);
            let _ = reply.send(());
        }
        PeerOp::RecordConnectionClosed { peer, conn_id, reason, at } => {
            peers.record_connection_closed(peer, conn_id, reason, at);
        }
        PeerOp::RecordConnectFailed { peer, at } => {
            peers.record_connect_failed(peer, at);
        }
        PeerOp::RecordLocalUseApplied { peer, conn_id, local_use, at } => {
            peers.record_local_use_applied(peer, conn_id, local_use, at);
        }
        PeerOp::MayInitiate { reply } => {
            let _ = reply.send(peers.may_initiate());
        }
        PeerOp::RecordRemoteUse { peer, conn_id, protocol, active, at, reply } => {
            let changed = peers.record_remote_use(peer, conn_id, protocol, active, at);
            let _ = reply.send(changed);
        }
        PeerOp::RecordKeepaliveSample { peer, rtt, at } => {
            peers.record_keepalive_sample(peer, rtt, at);
        }
        PeerOp::RecordSharedPeers { from, addrs, at, reply } => {
            let result = peers.record_shared_peers(&from, &addrs, at);
            let _ = reply.send(result);
        }
        PeerOp::RecordShareRequestServed { requester, amount, at } => {
            peers.record_share_request_served(requester, amount, at);
        }
        PeerOp::RecordUninteresting { peer, conn_id, after_rollback, at } => {
            peers.record_uninteresting(peer, conn_id, after_rollback, at);
        }
    }
}

fn dispatch_header(peers: &mut PeerPerformance, headers: &mut HeaderPerformance, op: HeaderOp) {
    match op {
        HeaderOp::RecordBlocksRequested { effect } => {
            headers.apply_blocks_requested(&effect.hashes, effect.requested_at);
        }
        HeaderOp::RecordPeersAsked { effect, reply } => {
            let telemetry = headers.apply_peers_asked(&effect.hashes, &effect.peers, effect.at);
            let _ = reply.send(telemetry);
        }
        HeaderOp::PruneBelow { effect, reply } => {
            peers.prune_below(effect.min_height);
            let telemetry = headers.apply_prune_below(effect.min_height, effect.now);
            let _ = reply.send(telemetry);
        }
        HeaderOp::RecordHeaderAbandoned { effect, reply } => {
            let telemetry = headers.apply_header_abandoned(&effect.hash, effect.now);
            let _ = reply.send(telemetry);
        }
        HeaderOp::RecordForkStarted { effect, reply } => {
            let telemetry = headers.apply_fork_started(effect.tip, effect.started_at);
            let _ = reply.send(telemetry);
        }
        HeaderOp::RecordBlockValid { effect, reply } => {
            let telemetry = headers.apply_block_valid(&effect.hash, effect.height, effect.now, effect.syncing);
            let _ = reply.send(telemetry);
        }
        HeaderOp::RecordBlockPruned { effect, reply } => {
            let telemetry = headers.apply_block_pruned(&effect.hash, effect.invalid, effect.now, effect.syncing);
            let _ = reply.send(telemetry);
        }
    }
}

fn dispatch_pace(pace: &mut SyncAdoptionPace, op: PaceOp) {
    match op {
        PaceOp::RecordSyncAdoption { effect } => {
            pace.record(effect.at, effect.live);
        }
        PaceOp::SyncAdoptionPace { effect, reply } => {
            let _ = reply.send(pace.is_catching_up_fast(effect.now));
        }
    }
}
