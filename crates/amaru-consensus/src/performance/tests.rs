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

//! Unit tests drive [`PeerPerformance`] / [`HeaderPerformance`] directly (no worker thread).
//! A small smoke test exercises the resource handle + external-effect path.

use std::{
    collections::BTreeSet,
    io::{self, Write},
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use amaru_kernel::{BlockHeight, HeaderHash, Peer, Point, Slot};
use amaru_observability::{CborConsoleEventFormat, console_field_formatter, debug, info};
use amaru_ouroboros::{
    CloseReason, ConnectionDirection, ConnectionId, ConnectionRecord, MiniProtocolUse, ObservedAt,
    PeerTrackingResource, RemoteInitiators, RemoteProtocol,
};
use amaru_pure_stage::{ExternalEffect, Instant, Resources};
use tracing_subscriber::{EnvFilter, prelude::*};

use super::{
    ClaimKind, HeaderLifecycleOutcome, HeaderPerformance, PeerPerformance, PeerShareFlags, Performance,
    ResourcePerformance, SHARE_REQUEST_WINDOW, SelectPeersParams, ShareRequestRecord,
};

fn t(secs: u64) -> Instant {
    Instant::at_offset(Duration::from_secs(secs), Duration::ZERO)
}

fn hash(byte: u8) -> HeaderHash {
    HeaderHash::from([byte; 32])
}

fn tip(byte: u8, height: u64) -> Point {
    Point::Specific(Slot::from(height), hash(byte), BlockHeight::from(height))
}

fn peer(name: &str) -> Peer {
    if let Ok(peer) = name.parse() {
        return peer;
    }
    let port = name.bytes().fold(10_000u16, |acc, b| acc.wrapping_add(u16::from(b)));
    Peer::for_test(port.max(1024))
}

fn select(need: Vec<HeaderHash>, max_peers: usize) -> SelectPeersParams {
    SelectPeersParams { need, max_peers, exclude: Vec::new(), now: t(100) }
}

// ---------------------------------------------------------------------------
// Peer coverage / scores (local PeerPerformance)
// ---------------------------------------------------------------------------

#[test]
fn intersect_only_covers_need_ending_at_intersect_not_unknown_child() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");
    let h1 = tip(1, 1);

    peers.record_intersection(alice, h1, None, t(1));

    assert!(peers.peer_covers_fragment(&alice, &[hash(1)]));
    assert!(!peers.peer_covers_fragment(&alice, &[hash(1), hash(2)]));
    assert!(!peers.peer_covers_fragment(&alice, &[hash(2)]));
}

#[test]
fn announce_chain_collapses_to_single_tip_and_covers_ancestors() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");

    peers.record_header_announcement(alice, tip(1, 1), None, t(1));
    peers.record_header_announcement(alice, tip(2, 2), Some(hash(1)), t(2));

    let snap = peers.snapshot(&alice).expect("alice present");
    assert_eq!(snap.tips.len(), 1);
    assert_eq!(snap.tips[0].hash, hash(2));

    assert!(peers.peer_covers_fragment(&alice, &[hash(1)]));
    assert!(peers.peer_covers_fragment(&alice, &[hash(1), hash(2)]));
    assert!(peers.peer_covers_fragment(&alice, &[hash(2)]));
}

#[test]
fn duplicate_announcers_both_selected() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");
    let bob = peer("bob");

    peers.record_header_announcement(alice, tip(1, 1), None, t(1));
    peers.record_header_announcement(bob, tip(1, 1), None, t(2));

    let set = peers.select_peers_for_fetch(select(vec![hash(1)], 2));
    assert!(!set.weak);
    assert_eq!(set.peers.len(), 2);
    assert!(set.peers.contains(&alice));
    assert!(set.peers.contains(&bob));

    let first = peers.first_announced_at(&hash(1)).expect("first announcer");
    assert_eq!(first.0, alice);
    assert_eq!(first.1, t(1));
}

#[test]
fn descendant_claim_covers_ancestor_via_parent_walk() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");

    peers.record_header_announcement(alice, tip(1, 1), None, t(1));
    peers.record_header_announcement(alice, tip(2, 2), Some(hash(1)), t(2));
    peers.record_header_announcement(alice, tip(3, 3), Some(hash(2)), t(3));

    assert!(peers.peer_covers_fragment(&alice, &[hash(1)]));
    assert!(peers.peer_covers_fragment(&alice, &[hash(2)]));
    assert!(peers.peer_covers_fragment(&alice, &[hash(1), hash(2), hash(3)]));
}

#[test]
fn parent_walk_stops_at_target_height_on_wrong_branch() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");
    let bob = peer("bob");

    for h in 1u8..=5 {
        let parent = (h > 1).then(|| hash(h - 1));
        peers.record_header_announcement(alice, tip(h, h as u64), parent, t(h as u64));
    }
    peers.record_intersection(bob, tip(30, 3), Some(hash(2)), t(10));

    assert!(!peers.peer_covers_fragment(&alice, &[hash(30)]));
    assert!(peers.peer_covers_fragment(&alice, &[hash(5)]));
    assert!(peers.peer_covers_fragment(&alice, &[hash(2)]));
}

#[test]
fn ancestor_only_claim_does_not_cover_descendant() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");

    peers.record_intersection(alice, tip(1, 1), None, t(1));

    assert!(peers.peer_covers_fragment(&alice, &[hash(1)]));
    assert!(!peers.peer_covers_fragment(&alice, &[hash(2)]));
    assert!(!peers.peer_covers_fragment(&alice, &[hash(1), hash(2)]));
}

#[test]
fn intersect_at_tip_of_need_covers_full_fragment_via_index() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");

    peers.record_intersection(alice, tip(3, 3), Some(hash(2)), t(1));

    assert!(peers.peer_covers_fragment(&alice, &[hash(1), hash(2), hash(3)]));
    assert!(peers.peer_covers_fragment(&alice, &[hash(3)]));
}

#[test]
fn rollback_drops_fork_tip_and_restores_point() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");

    peers.record_header_announcement(alice, tip(1, 1), None, t(1));
    peers.record_header_announcement(alice, tip(2, 2), Some(hash(1)), t(2));
    peers.record_header_announcement(alice, tip(10, 3), Some(hash(1)), t(3));

    let before = peers.snapshot(&alice).expect("alice");
    assert!(!before.tips.is_empty());

    peers.record_rollback(alice, tip(1, 1), None, t(4));

    let after = peers.snapshot(&alice).expect("alice");
    assert!(after.tips.iter().any(|c| c.hash == hash(1)));
    assert!(!after.tips.iter().any(|c| c.hash == hash(2)));
    assert!(!after.tips.iter().any(|c| c.hash == hash(10)));
    assert!(peers.peer_covers_fragment(&alice, &[hash(1)]));
    assert!(!peers.peer_covers_fragment(&alice, &[hash(2)]));
}

#[test]
fn prune_removes_old_tips_but_retains_scores() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");

    peers.record_header_announcement(alice, tip(1, 1), None, t(1));
    peers.record_block_delivery(alice, hash(1), BlockHeight::from(1), None, t(2), Duration::from_millis(50), 90_000);

    let scores_before = peers.scores(&alice);
    assert!(scores_before.block_response_ewma.is_some());
    assert_eq!(scores_before.fetch_successes, 1);

    peers.prune_below(BlockHeight::from(5));

    let snap = peers.snapshot(&alice).expect("alice kept for scores");
    assert!(snap.tips.is_empty());
    assert!(!peers.peer_covers_fragment(&alice, &[hash(1)]));
    assert_eq!(peers.scores(&alice).fetch_successes, 1);
    assert!(peers.scores(&alice).block_response_ewma.is_some());
}

#[test]
fn clear_availability_keeps_scores_and_share_flags() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");

    peers.record_advertisability(alice, true, t(0));
    peers.record_header_announcement(alice, tip(1, 1), None, t(1));
    peers.record_block_delivery(alice, hash(1), BlockHeight::from(1), None, t(2), Duration::from_millis(20), 1000);

    peers.clear_availability(&alice);
    assert!(!peers.peer_covers_fragment(&alice, &[hash(1)]));
    assert_eq!(peers.scores(&alice).fetch_successes, 1);
    assert!(peers.direct_claimants(&hash(1)).is_empty());
    assert_eq!(
        peers.share_flags(&alice),
        Some(PeerShareFlags { ever_connected: true, advertisable: true, failure_count: 0, adversarial: false })
    );
    assert!(peers.ok_for_sharing(&alice, t(10)));
}

#[test]
fn peer_adversarial_keeps_reputation_stub_clears_claims_and_scores() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");

    peers.record_advertisability(alice, true, t(0));
    peers.record_connection_failure(alice, t(1));
    peers.record_header_announcement(alice, tip(1, 1), None, t(2));
    peers.record_block_delivery(alice, hash(1), BlockHeight::from(1), None, t(3), Duration::from_millis(20), 1000);

    peers.mark_adversarial(&alice, t(4));

    let snap = peers.snapshot(&alice).expect("stub retained after adversarial mark");
    assert!(snap.tips.is_empty());
    assert_eq!(snap.scores.fetch_successes, 0);
    assert!(snap.scores.block_response_ewma.is_none());
    assert_eq!(
        snap.share,
        PeerShareFlags { ever_connected: true, advertisable: true, failure_count: 1, adversarial: true }
    );
    assert!(!peers.ok_for_sharing(&alice, t(10)));
    assert!(peers.direct_claimants(&hash(1)).is_empty());
}

#[test]
fn advertisability_latest_handshake_wins() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");

    peers.record_advertisability(alice, true, t(1));
    assert!(peers.ok_for_sharing(&alice, t(10)));

    peers.record_advertisability(alice, false, t(2));
    assert_eq!(
        peers.share_flags(&alice),
        Some(PeerShareFlags { ever_connected: true, advertisable: false, failure_count: 0, adversarial: false })
    );
    assert!(!peers.ok_for_sharing(&alice, t(10)));

    peers.record_advertisability(alice, true, t(3));
    assert!(peers.ok_for_sharing(&alice, t(10)));
}

#[test]
fn connection_failure_blocks_sharing_until_malus_decays() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");

    assert!(!peers.ok_for_sharing(&alice, t(10)));

    peers.record_advertisability(alice, true, t(1));
    assert!(peers.ok_for_sharing(&alice, t(10)));

    peers.record_connection_failure(alice, t(2));
    assert_eq!(peers.share_flags(&alice).map(|f| f.failure_count), Some(1));
    assert!(!peers.ok_for_sharing(&alice, t(2)));

    peers.record_connection_failure(alice, t(3));
    assert_eq!(peers.share_flags(&alice).map(|f| f.failure_count), Some(2));
    // Still high shortly after failures.
    assert!(!peers.ok_for_sharing(&alice, t(3)));
}

#[test]
fn connection_failure_only_does_not_mark_ever_connected() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");

    peers.record_connection_failure(alice, t(1));

    assert_eq!(
        peers.share_flags(&alice),
        Some(PeerShareFlags { ever_connected: false, advertisable: false, failure_count: 1, adversarial: false })
    );
    assert!(!peers.ok_for_sharing(&alice, t(10)));
}

#[test]
fn connection_malus_decays_with_half_life_without_new_samples() {
    use crate::performance::{CONNECT_FAIL_IMPULSE, DEFAULT_PEER_MALUS_HALF_LIFE, malus_at};

    let mut peers = PeerPerformance::new();
    let alice = peer("alice");
    peers.record_connection_failure(alice, t(0));

    let hl = peers.half_life_for(&alice);
    assert_eq!(hl, DEFAULT_PEER_MALUS_HALF_LIFE);
    let m0 = malus_at(CONNECT_FAIL_IMPULSE, Some(t(0)), t(0), hl);
    assert!((m0 - CONNECT_FAIL_IMPULSE).abs() < 1e-9);

    // One half-life later (no intervening events): malus halves via lazy evaluate.
    let later = t(hl.as_secs());
    let m = malus_at(CONNECT_FAIL_IMPULSE, Some(t(0)), later, hl);
    assert!((m - CONNECT_FAIL_IMPULSE * 0.5).abs() < 1e-9, "m={m}");
}

#[test]
fn outbound_selection_prefers_never_connected_over_fresh_failure() {
    use std::collections::BTreeSet;

    use crate::performance::{PeerMix, SelectOutboundParams};

    let good = peer("good:1");
    let bad = peer("bad:1");
    let mut peers = PeerPerformance::with_sources(
        BTreeSet::from([good, bad]).into_iter().map(amaru_kernel::PeerCandidate::from).collect(),
        BTreeSet::new(),
        BTreeSet::new(),
        PeerMix::parse("static~1").unwrap(),
    );
    peers.record_connection_failure(bad, t(1));

    // A fresh failure ranks behind a never-connected peer, so the one open slot stays with the healthy peer.
    for i in 0..20u8 {
        let seed = [i; 32];
        let picked = peers.select_outbound(SelectOutboundParams {
            open: 1,
            excluded: BTreeSet::new(),
            eligible_inbound: 0,
            seed,
            now: t(1),
        });
        assert_eq!(
            picked.outbound.iter().map(|p| p.candidate.as_peer()).collect::<Vec<_>>(),
            vec![Some(good)],
            "seed {i} offered the failed peer"
        );
    }
}

#[test]
fn failed_hostname_is_offered_when_nothing_better_remains() {
    use std::collections::BTreeSet;

    use amaru_kernel::PeerCandidate;

    use crate::performance::{PeerMix, PeerSource, SelectOutboundParams};

    let host = PeerCandidate::host("relay.example".parse().unwrap(), 3001);
    let resolved = peer("10.9.9.9:3001");
    let mut peers = PeerPerformance::with_sources(
        BTreeSet::new(),
        BTreeSet::from([host.clone()]),
        BTreeSet::new(),
        PeerMix::parse("snapshot~1@10s").unwrap(),
    );
    peers.note_dial(PeerSource::Snapshot, &host, resolved, t(0));
    peers.record_connection_failure(resolved, t(0));

    let picked = peers.select_outbound(SelectOutboundParams {
        open: 1,
        excluded: BTreeSet::new(),
        eligible_inbound: 0,
        seed: [0x42; 32],
        now: t(0),
    });
    assert_eq!(picked.outbound.len(), 1, "malus must not drop the only candidate");
    assert_eq!(picked.outbound[0].candidate, host);
}

#[test]
fn open_slots_fill_worse_scores_after_better_ones() {
    use std::collections::BTreeSet;

    use amaru_kernel::PeerCandidate;

    use crate::performance::{PeerMix, PeerSource, SelectOutboundParams};

    let fresh = PeerCandidate::host("fresh.example".parse().unwrap(), 3001);
    let failed = PeerCandidate::host("failed.example".parse().unwrap(), 3001);
    let resolved = peer("10.9.9.9:3001");
    let mut peers = PeerPerformance::with_sources(
        BTreeSet::new(),
        BTreeSet::from([fresh.clone(), failed.clone()]),
        BTreeSet::new(),
        PeerMix::parse("snapshot~1@10s").unwrap(),
    );
    peers.note_dial(PeerSource::Snapshot, &failed, resolved, t(0));
    peers.record_connection_failure(resolved, t(0));

    let picked = peers.select_outbound(SelectOutboundParams {
        open: 2,
        excluded: BTreeSet::new(),
        eligible_inbound: 0,
        seed: [0x42; 32],
        now: t(0),
    });
    assert_eq!(picked.outbound.len(), 2);
    assert_eq!(picked.outbound[0].candidate, fresh);
    assert_eq!(picked.outbound[1].candidate, failed);
}

#[test]
fn outbound_selection_picks_unresolved_host() {
    use std::collections::BTreeSet;

    use amaru_kernel::PeerCandidate;

    use crate::performance::{OutboundPick, PeerMix, PeerSource, SelectOutboundParams};

    let host = PeerCandidate::host("relay.example".parse().unwrap(), 3001);
    let peers = PeerPerformance::with_sources(
        BTreeSet::from([host.clone()]),
        BTreeSet::new(),
        BTreeSet::new(),
        PeerMix::parse("static~1").unwrap(),
    );
    let picked = peers.select_outbound(SelectOutboundParams {
        open: 1,
        excluded: BTreeSet::new(),
        eligible_inbound: 0,
        seed: [0x42; 32],
        now: t(1),
    });
    assert_eq!(picked.inbound, 0);
    assert_eq!(picked.outbound, vec![OutboundPick { candidate: host, origin: PeerSource::Static }]);
}

#[test]
fn outbound_selection_skips_excluded_unresolved_host() {
    use std::collections::BTreeSet;

    use amaru_kernel::PeerCandidate;

    use crate::performance::{PeerMix, SelectOutboundParams};

    let host = PeerCandidate::host("relay.example".parse().unwrap(), 3001);
    let peers = PeerPerformance::with_sources(
        BTreeSet::from([host.clone()]),
        BTreeSet::new(),
        BTreeSet::new(),
        PeerMix::parse("static~1").unwrap(),
    );
    let picked = peers.select_outbound(SelectOutboundParams {
        open: 1,
        excluded: BTreeSet::from([host]),
        eligible_inbound: 0,
        seed: [0x42; 32],
        now: t(1),
    });
    assert!(picked.outbound.is_empty());
    assert_eq!(picked.inbound, 0);
}

#[test]
fn note_dial_keeps_hostname_in_pool_and_marks_origin() {
    use std::collections::BTreeSet;

    use amaru_kernel::PeerCandidate;

    use crate::performance::{OutboundPick, PeerMix, PeerSource, SelectOutboundParams};

    let host = PeerCandidate::host("relay.example".parse().unwrap(), 3001);
    let resolved = peer("10.9.9.9:3001");
    let mut peers = PeerPerformance::with_sources(
        BTreeSet::from([host.clone()]),
        BTreeSet::new(),
        BTreeSet::new(),
        PeerMix::parse("static~1").unwrap(),
    );
    peers.note_dial(PeerSource::Static, &host, resolved, t(1));
    assert!(peers.is_static_peer(&resolved));
    let picked = peers.select_outbound(SelectOutboundParams {
        open: 1,
        excluded: BTreeSet::new(),
        eligible_inbound: 0,
        seed: [0x42; 32],
        now: t(1),
    });
    assert_eq!(picked.outbound, vec![OutboundPick { candidate: host, origin: PeerSource::Static }]);
}

#[test]
fn inbound_mix_slots_are_allotted_not_dialed() {
    use std::collections::BTreeSet;

    use crate::performance::{PeerMix, PeerSource, SelectOutboundParams};

    let static_peer = peer("10.0.0.1:1");
    let peers = PeerPerformance::with_sources(
        BTreeSet::from([static_peer]).into_iter().map(amaru_kernel::PeerCandidate::from).collect(),
        BTreeSet::new(),
        BTreeSet::new(),
        PeerMix::parse("inbound~1, static~1").unwrap(),
    );
    let picked = peers.select_outbound(SelectOutboundParams {
        open: 2,
        excluded: BTreeSet::new(),
        eligible_inbound: 5,
        seed: [0x11; 32],
        now: t(1),
    });
    assert_eq!(picked.inbound, 1);
    assert_eq!(picked.outbound.len(), 1);
    assert_eq!(picked.outbound[0].origin, PeerSource::Static);
}

#[test]
fn ranking_prefers_faster_delivery() {
    let mut peers = PeerPerformance::new();
    let fast = peer("fast");
    let slow = peer("slow");
    let partial = peer("partial");

    peers.record_header_announcement(fast, tip(1, 1), None, t(1));
    peers.record_header_announcement(fast, tip(2, 2), Some(hash(1)), t(2));
    peers.record_block_delivery(
        fast,
        hash(2),
        BlockHeight::from(2),
        Some(hash(1)),
        t(3),
        Duration::from_millis(10),
        90_000,
    );

    peers.record_header_announcement(slow, tip(1, 1), None, t(1));
    peers.record_header_announcement(slow, tip(2, 2), Some(hash(1)), t(2));
    peers.record_block_delivery(
        slow,
        hash(2),
        BlockHeight::from(2),
        Some(hash(1)),
        t(4),
        Duration::from_secs(2),
        90_000,
    );

    peers.record_header_announcement(partial, tip(1, 1), None, t(1));

    let set = peers.select_peers_for_fetch(select(vec![hash(1), hash(2)], 3));
    assert!(!set.weak);
    assert_eq!(set.peers, vec![fast, slow]);
    assert!(!set.peers.contains(&partial));
}

#[test]
fn prefix_only_peer_not_selected_for_range() {
    let mut peers = PeerPerformance::new();
    let prefix = peer("prefix");
    let full = peer("full");

    peers.record_header_announcement(prefix, tip(1, 1), None, t(1));
    peers.record_header_announcement(full, tip(1, 1), None, t(1));
    peers.record_header_announcement(full, tip(2, 2), Some(hash(1)), t(2));

    let set = peers.select_peers_for_fetch(select(vec![hash(1), hash(2)], 5));
    assert_eq!(set.peers, vec![full]);
    assert!(!set.peers.contains(&prefix));
}

#[test]
fn cold_start_empty_map_returns_weak_empty_selection() {
    let peers = PeerPerformance::new();
    let set = peers.select_peers_for_fetch(select(vec![hash(1)], 5));
    assert!(set.weak);
    assert!(set.peers.is_empty());
}

#[test]
fn after_intersect_selection_becomes_non_empty() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");

    let empty = peers.select_peers_for_fetch(select(vec![hash(5)], 5));
    assert!(empty.weak);

    peers.record_intersection(alice, tip(5, 5), Some(hash(4)), t(1));

    let set = peers.select_peers_for_fetch(select(vec![hash(5)], 5));
    assert!(!set.weak);
    assert_eq!(set.peers, vec![alice]);
}

#[test]
fn later_announcer_is_selected_when_the_first_is_excluded() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");
    let bob = peer("bob");

    peers.record_header_announcement(alice, tip(1, 1), None, t(1));
    let first = peers.select_peers_for_fetch(select(vec![hash(1)], 3));
    assert!(!first.weak);
    assert_eq!(first.peers, vec![alice]);

    peers.record_header_announcement(bob, tip(1, 1), None, t(2));
    let mut again = select(vec![hash(1)], 3);
    again.exclude = vec![alice];
    let second = peers.select_peers_for_fetch(again);
    assert!(!second.weak);
    assert_eq!(second.peers, vec![bob]);

    let mut nobody = select(vec![hash(1)], 3);
    nobody.exclude = vec![alice, bob];
    let third = peers.select_peers_for_fetch(nobody);
    assert!(third.weak);
    assert!(third.peers.is_empty());
}

#[test]
fn max_peers_bounds_selection() {
    let mut peers = PeerPerformance::new();
    for i in 0..10u8 {
        let p = peer(&format!("p{i}"));
        peers.record_header_announcement(p, tip(1, 1), None, t(1));
    }
    let set = peers.select_peers_for_fetch(select(vec![hash(1)], 3));
    assert_eq!(set.peers.len(), 3);
    assert!(!set.weak);
}

#[test]
fn churn_ranks_unreliable_peers_first() {
    let mut peers = PeerPerformance::new();
    let good = peer("good");
    let bad = peer("bad");

    peers.record_header_announcement(good, tip(1, 1), None, t(1));
    peers.record_block_delivery(good, hash(1), BlockHeight::from(1), None, t(2), Duration::from_millis(10), 1000);

    peers.record_header_announcement(bad, tip(1, 1), None, t(1));
    peers.record_fetch_failure(std::slice::from_ref(&bad), t(3));
    peers.record_fetch_failure(std::slice::from_ref(&bad), t(4));

    let ranked = peers.rank_peers_for_churn(&[good, bad], t(5));
    assert_eq!(ranked[0].peer, bad);
    assert_eq!(ranked[1].peer, good);
}

#[test]
fn claim_kind_strength_prefers_delivery_over_intersection() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");

    peers.record_intersection(alice, tip(1, 1), None, t(1));
    peers.record_block_delivery(alice, hash(1), BlockHeight::from(1), None, t(2), Duration::from_millis(5), 100);

    let claimants = peers.direct_claimants(&hash(1));
    assert_eq!(claimants.len(), 1);
    assert_eq!(claimants[0].2, ClaimKind::BlockDelivery);
}

#[test]
fn header_lag_records_zero_for_first_announcer_and_delay_for_late() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");
    let bob = peer("bob");

    peers.record_header_announcement(alice, tip(1, 1), None, t(1));
    peers.record_header_announcement(bob, tip(1, 1), None, t(3));

    assert_eq!(peers.scores(&alice).header_lag_ewma, Some(Duration::ZERO));
    assert_eq!(peers.scores(&bob).header_lag_ewma, Some(Duration::from_secs(2)));
}

// ---------------------------------------------------------------------------
// Header lifecycle (local HeaderPerformance)
// ---------------------------------------------------------------------------

#[test]
fn header_received_and_peer_claim_are_independent_maps() {
    let mut peers = PeerPerformance::new();
    let mut headers = HeaderPerformance::new();
    let alice = peer("alice");

    peers.record_header_announcement(alice, tip(1, 1), None, t(1));
    headers.apply_header_received(alice, tip(1, 1), t(1), 1_000, Duration::ZERO, false);

    assert_eq!(headers.lifecycle_count(), 1);
    assert!(peers.peer_covers_fragment(&alice, &[hash(1)]));
    assert_eq!(peers.first_announced_at(&hash(1)).map(|(p, _)| p), Some(alice));
}

#[test]
fn first_header_announcer_peer_is_retained() {
    let mut headers = HeaderPerformance::new();
    let alice = peer("alice");
    let bob = peer("bob");

    headers.apply_header_received(alice, tip(1, 1), t(1), 1_000, Duration::ZERO, false);
    // Later announcer must not overwrite the first peer or slot interval.
    headers.apply_header_received(bob, tip(1, 1), t(2), 9_999, Duration::ZERO, false);

    assert_eq!(headers.first_announcer(&hash(1)), Some(alice));
    assert_eq!(headers.slot_start_to_header_micros(&hash(1)), Some(1_000));
}

#[test]
fn blocks_requested_and_downloaded_then_valid_closes_lifecycle() {
    let mut headers = HeaderPerformance::new();
    let alice = peer("alice");
    headers.apply_header_received(alice, tip(1, 1), t(1), 500, Duration::ZERO, false);
    headers.apply_blocks_requested(&[hash(1)], t(2));
    let received = headers.apply_block_downloaded(alice, &hash(1), BlockHeight::from(1), t(3));
    assert!(matches!(
        &received[..],
        [super::HeaderTelemetry::Received { rank: 1, peer, .. }] if *peer == alice
    ));
    assert_eq!(headers.lifecycle_count(), 1);
    let telemetry = headers.apply_block_valid(&hash(1), BlockHeight::from(1), t(4), false);
    assert_eq!(headers.lifecycle_count(), 0);
    assert_eq!(telemetry.len(), 2);
    assert!(matches!(
        &telemetry[0],
        super::HeaderTelemetry::Lifecycle {
            outcome: HeaderLifecycleOutcome::ValidBlock,
            slot_start_to_header_micros: Some(500),
            ..
        }
    ));
    assert!(matches!(&telemetry[1], super::HeaderTelemetry::Adopted { peer: Some(peer), .. } if *peer == alice));
}

#[test]
fn header_rejected_does_not_require_lifecycle_entry() {
    let telemetry = HeaderPerformance::apply_header_rejected(HeaderLifecycleOutcome::DuplicateHeader);
    assert!(matches!(
        telemetry,
        super::HeaderTelemetry::Lifecycle { outcome: HeaderLifecycleOutcome::DuplicateHeader, hash: None, .. }
    ));
}

#[test]
fn fork_started_and_closed_on_valid_block() {
    let mut headers = HeaderPerformance::new();
    headers.apply_header_received(peer("alice"), tip(1, 1), t(1), 0, Duration::ZERO, false);
    assert!(headers.apply_fork_started(tip(1, 1), t(1)).is_empty());
    assert!(headers.has_fork_switch(&hash(1)));
    let telemetry = headers.apply_block_valid(&hash(1), BlockHeight::from(1), t(2), false);
    assert!(!headers.has_fork_switch(&hash(1)));
    assert_eq!(telemetry.len(), 3); // lifecycle + adopted + fork switch
}

#[test]
fn slot_start_metric_omitted_while_syncing() {
    let mut headers = HeaderPerformance::new();
    headers.apply_header_received(peer("alice"), tip(1, 1), t(1), 42_000, Duration::ZERO, false);
    let telemetry = headers.apply_block_valid(&hash(1), BlockHeight::from(1), t(2), true);
    assert_eq!(headers.lifecycle_count(), 0);
    assert!(matches!(
        &telemetry[0],
        super::HeaderTelemetry::Lifecycle {
            outcome: HeaderLifecycleOutcome::ValidBlock,
            slot_start_to_header_micros: None,
            ..
        }
    ));
}

#[test]
fn announcement_logs_three_peers_and_deliveries_keep_arrival_order() {
    let mut headers = HeaderPerformance::new();
    let peers: Vec<Peer> = (1..=4).map(|port| Peer::for_test(3000 + port)).collect();
    let announced: Vec<_> = peers
        .iter()
        .flat_map(|peer| headers.apply_header_received(*peer, tip(1, 1), t(1), 0, Duration::ZERO, false))
        .collect();
    assert_eq!(announced.len(), 3);
    assert!(matches!(&announced[0], super::HeaderTelemetry::Announced { rank: 1, peer, .. } if *peer == peers[0]));
    assert!(matches!(&announced[2], super::HeaderTelemetry::Announced { rank: 3, peer, .. } if *peer == peers[2]));
    assert!(headers.apply_header_received(peers[0], tip(1, 1), t(2), 0, Duration::ZERO, false).is_empty());

    let first = headers.apply_block_downloaded(peers[2], &hash(1), BlockHeight::from(1), t(3));
    assert!(headers.apply_block_downloaded(peers[2], &hash(1), BlockHeight::from(1), t(4)).is_empty());
    let second = headers.apply_block_downloaded(peers[0], &hash(1), BlockHeight::from(1), t(5));
    assert!(matches!(&first[..], [super::HeaderTelemetry::Received { rank: 1, peer, .. }] if *peer == peers[2]));
    assert!(matches!(&second[..], [super::HeaderTelemetry::Received { rank: 2, peer, .. }] if *peer == peers[0]));

    let adopted = headers.apply_block_valid(&hash(1), BlockHeight::from(1), t(6), false);
    assert!(
        matches!(adopted.last(), Some(super::HeaderTelemetry::Adopted { peer: Some(peer), .. }) if *peer == peers[2])
    );
}

#[test]
fn already_stored_header_does_not_open_a_rank_one_announcement() {
    let mut headers = HeaderPerformance::new();
    let alice = peer("alice");
    let bob = peer("bob");

    let first = headers.apply_header_received(alice, tip(1, 1), t(1), 1_000, Duration::ZERO, false);
    assert!(matches!(&first[..], [super::HeaderTelemetry::Announced { rank: 1, .. }]));

    // A later peer extends the open list and leaves the first announcer's interval in place.
    let second = headers.apply_header_received(bob, tip(1, 1), t(2), 9_000, Duration::ZERO, true);
    assert!(matches!(&second[..], [super::HeaderTelemetry::Announced { rank: 2, peer, .. }] if *peer == bob));
    assert_eq!(headers.first_announcer(&hash(1)), Some(alice));
    assert_eq!(headers.slot_start_to_header_micros(&hash(1)), Some(1_000));

    headers.apply_block_valid(&hash(1), BlockHeight::from(1), t(3), false);
    assert!(
        headers.apply_header_received(peer("carol"), tip(1, 1), t(4), 1, Duration::ZERO, true).is_empty(),
        "an adopted header is not announced again"
    );
    assert!(headers.apply_header_received(peer("dave"), tip(1, 1), t(4), 1, Duration::ZERO, false).is_empty());
    headers.apply_prune_below(BlockHeight::from(2), t(5));
    assert!(headers.apply_header_received(peer("carol"), tip(1, 1), t(5), 1, Duration::ZERO, true).is_empty());
    assert_eq!(headers.lifecycle_count(), 0);

    // A stored header this node never collected announcers for stays silent.
    assert!(headers.apply_header_received(alice, tip(9, 9), t(6), 1, Duration::ZERO, true).is_empty());
    assert_eq!(headers.lifecycle_count(), 0);
}

#[test]
fn blockperf_latencies_follow_slot_onset_and_the_peer_who_was_asked() {
    let mut headers = HeaderPerformance::new();
    let alice = peer("alice");
    let bob = peer("bob");
    let onset = Duration::from_millis(500);

    let announced = headers.apply_header_received(alice, tip(1, 1), t(1), 500_000, onset, false);
    assert!(matches!(&announced[..], [super::HeaderTelemetry::Announced { slot_latency_ms: Some(500), rank: 1, .. }]));
    let later = headers.apply_header_received(bob, tip(1, 1), t(2), 0, onset, true);
    assert!(matches!(&later[..], [super::HeaderTelemetry::Announced { slot_latency_ms: Some(1_500), rank: 2, .. }]));

    let asked = headers.apply_peers_asked(&[hash(1)], &[alice], t(3));
    assert!(matches!(&asked[..], [super::HeaderTelemetry::Requested { slot_latency_ms: Some(2_500), .. }]));
    // A repeat ask does not move Alice's request time. Bob is a later, staggered ask.
    headers.apply_peers_asked(&[hash(1)], &[alice, bob], t(4));

    let from_alice = headers.apply_block_downloaded(alice, &hash(1), BlockHeight::from(1), t(5));
    assert!(matches!(
        &from_alice[..],
        [super::HeaderTelemetry::Received { slot_latency_ms: Some(4_500), fetch_latency_ms: Some(2_000), rank: 1, .. }]
    ));
    let from_bob = headers.apply_block_downloaded(bob, &hash(1), BlockHeight::from(1), t(5));
    assert!(matches!(&from_bob[..], [super::HeaderTelemetry::Received { fetch_latency_ms: Some(1_000), rank: 2, .. }]));

    let adopted = headers.apply_block_valid(&hash(1), BlockHeight::from(1), t(6), false);
    assert!(matches!(adopted.last(), Some(super::HeaderTelemetry::Adopted { slot_latency_ms: Some(5_500), .. })));
}

#[test]
fn prune_below_closes_open_lifecycles_as_pruned() {
    let mut headers = HeaderPerformance::new();
    headers.apply_header_received(peer("alice"), tip(1, 1), t(1), 0, Duration::ZERO, false);
    headers.apply_header_received(peer("alice"), tip(5, 5), t(2), 0, Duration::ZERO, false);
    assert_eq!(headers.lifecycle_count(), 2);
    let pruned = headers.apply_prune_below(BlockHeight::from(5), t(3));
    assert_eq!(headers.lifecycle_count(), 1);
    assert_eq!(pruned.len(), 1);
    assert!(matches!(&pruned[0], super::HeaderTelemetry::Lifecycle { outcome: HeaderLifecycleOutcome::Pruned, .. }));
    headers.apply_prune_below(BlockHeight::from(6), t(4));
    assert_eq!(headers.lifecycle_count(), 0);
}

// ---------------------------------------------------------------------------
// Resource handle + external effects (smoke)
// ---------------------------------------------------------------------------

#[test]
fn effect_constructors_mutate_resource_via_run() {
    let perf = Arc::new(Performance::new());
    let resources = Resources::default();
    resources.put::<ResourcePerformance>(perf.clone());

    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();

    let alice = peer("alice");
    let effect = Performance::record_intersection(alice, tip(7, 7), None, t(1));
    let _ = rt.block_on(Box::new(effect).run(resources.clone()));

    let select_effect = Performance::select_peers_for_fetch(select(vec![hash(7)], 5));
    let response = rt.block_on(Box::new(select_effect).run(resources));
    let set = *response.cast::<super::FetchPeerSet>().expect("FetchPeerSet response");
    assert!(!set.weak);
    assert_eq!(set.peers, vec![alice]);
}

#[test]
fn dropping_last_performance_handle_joins_worker() {
    // Spawns a worker; when the last handle is dropped the op channel closes and Drop joins the thread.
    // If join were missing or deadlocked, this test would hang or leak a thread under tools like TSAN.
    let perf = Performance::new();
    let clone = perf.clone();
    drop(perf);
    drop(clone);
}

#[test]
fn shutdown_callback_reports_worker_panic() {
    let mut perf = Performance::new();
    let original_worker = perf.shutdown_callback();
    perf.worker = Arc::new(super::WorkerGuard {
        join: parking_lot::Mutex::new(Some(std::thread::spawn(|| panic!("performance worker failed")))),
    });
    let join = perf.shutdown_callback();
    drop(perf);

    original_worker().unwrap();
    assert!(join().is_err());
}

/// Queue-depth rate limiting calls [`tokio::time::Instant::now`] from stage threads that may
/// not be running a Tokio runtime. This guards that assumption.
#[test]
fn tokio_time_instant_now_works_outside_tokio_thread() {
    let result = std::panic::catch_unwind(|| {
        let a = tokio::time::Instant::now();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let b = tokio::time::Instant::now();
        assert!(b >= a);
        assert!(b.duration_since(a) >= std::time::Duration::from_millis(1));
    });
    assert!(result.is_ok(), "tokio::time::Instant::now panicked outside a Tokio runtime/thread: {result:?}");
}

#[derive(Clone)]
struct MemWriter(Arc<Mutex<Vec<u8>>>);

impl Write for MemWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().expect("log buffer").extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Drive the same effect path the stages use: four announcers, a fetch request, two deliveries,
/// adoption, plus ordinary consensus logs that must stay selectable apart from block propagation.
fn capture_blockperf(filter: &str, live: bool) -> String {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let writer = MemWriter(Arc::clone(&buf));
    let layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .event_format(CborConsoleEventFormat::new().with_ansi(false))
        .fmt_fields(console_field_formatter())
        .with_writer(move || writer.clone())
        .with_filter(EnvFilter::new(filter));
    let subscriber = tracing_subscriber::registry().with(layer);

    amaru_observability::tracing::subscriber::with_default(subscriber, || {
        let resources = Resources::default();
        resources.put::<ResourcePerformance>(Arc::new(Performance::new()));
        if live {
            resources.put(crate::consensus_mode::ConsensusMode::Live);
        }
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
        let body = hash(9);
        let point = tip(9, 4);
        let peers: Vec<Peer> = (1..=4).map(|port| Peer::for_test(3000 + port)).collect();
        for peer in &peers {
            let effect = Performance::record_header_announcement(*peer, point, None, t(1), 0, Duration::ZERO, false);
            rt.block_on(Box::new(effect).run(resources.clone()));
        }
        let asked = Performance::record_peers_asked(vec![body], peers[..2].to_vec(), t(2));
        rt.block_on(Box::new(asked).run(resources.clone()));
        for peer in [peers[1], peers[0]] {
            let effect = Performance::record_block_delivery(
                peer,
                body,
                BlockHeight::from(4),
                None,
                t(2),
                Duration::from_millis(5),
                100,
            );
            rt.block_on(Box::new(effect).run(resources.clone()));
        }
        let effect = Performance::record_block_valid(body, point.block_height(), t(3), false);
        rt.block_on(Box::new(effect).run(resources.clone()));
        info!(consensus::blocks::PAUSED, req_id = 1u64);
        debug!(consensus::blocks::TIMEOUT, req_id = 1u64);
    });

    String::from_utf8(buf.lock().expect("log buffer").clone()).expect("utf-8 log")
}

#[test]
fn env_filter_selects_exactly_the_four_blockperf_events() {
    let only = capture_blockperf("off,amaru::blockperf=debug", false);
    let lines: Vec<_> = only.lines().filter(|line| !line.is_empty()).collect();
    assert!(lines.iter().all(|line| line.contains("amaru::blockperf")), "unexpected lines:\n{only}");
    for name in ["header.announced", "block.requested", "block.received", "block.adopted"] {
        assert!(lines.iter().any(|line| line.contains(name)), "missing {name}:\n{only}");
    }
    assert!(only.contains("slot_latency_ms"), "{only}");
    assert!(lines.iter().any(|line| line.contains("block.received") && line.contains("fetch_latency_ms")), "{only}");
    assert_eq!(lines.iter().filter(|line| line.contains("header.announced")).count(), 3, "{only}");
    assert!(only.contains(r#"peers="127.0.0.1:3001,127.0.0.1:3002""#), "{only}");
    assert!(only.contains(r#"peer="127.0.0.1:3001""#), "{only}");
    assert!(only.contains(r#"peer="127.0.0.1:3002""#), "{only}");
    assert!(only.contains(r#"peer="127.0.0.1:3003""#), "{only}");
    assert!(!only.contains("127.0.0.1:3004"), "{only}");
    assert!(only.contains("rank=1") && only.contains("rank=2") && only.contains("rank=3"), "{only}");
    assert!(!only.contains("rank=4"), "{only}");
    assert!(
        lines.iter().any(|line| {
            line.contains("block.received") && line.contains(r#"peer="127.0.0.1:3002""#) && line.contains("rank=1")
        }),
        "{only}"
    );
    assert!(
        lines.iter().any(|line| {
            line.contains("block.received") && line.contains(r#"peer="127.0.0.1:3001""#) && line.contains("rank=2")
        }),
        "{only}"
    );
    assert!(
        lines.iter().any(|line| line.contains("block.adopted") && line.contains(r#"peer="127.0.0.1:3002""#)),
        "{only}"
    );
    assert!(!only.contains("blocks.paused"), "{only}");
    assert!(!only.contains("blocks.timeout"), "{only}");
    assert!(!only.contains("perf.header.lifecycle"), "{only}");

    // The span-name syntax does not select these events: a bracketed name matches the current span.
    let legacy = capture_blockperf("info,amaru::consensus[perf.header.lifecycle{peer}]", false);
    assert!(!legacy.contains("header.announced"), "{legacy}");
    assert!(!legacy.contains("block.requested"), "{legacy}");
    assert!(!legacy.contains("block.received"), "{legacy}");
    assert!(!legacy.contains("block.adopted"), "{legacy}");
    assert!(legacy.contains("blocks.paused"), "{legacy}");

    let with_info = capture_blockperf("info,amaru::blockperf=debug", false);
    assert!(with_info.lines().any(|line| line.contains(" DEBUG ") && line.contains("header.announced")), "{with_info}");

    let live = capture_blockperf("info,amaru::blockperf=info", true);
    assert!(live.lines().any(|line| line.contains(" INFO ") && line.contains("header.announced")), "{live}");
    assert!(live.lines().any(|line| line.contains(" INFO ") && line.contains("block.adopted")), "{live}");
    assert!(!live.lines().any(|line| line.contains(" DEBUG ") && line.contains("amaru::blockperf")), "{live}");
    assert!(with_info.contains("blocks.paused"), "{with_info}");
    assert!(!with_info.contains("blocks.timeout"), "{with_info}");
    assert!(with_info.contains("header.announced") && with_info.contains("block.adopted"), "{with_info}");
}

fn observed(elapsed_secs: u64, offset_secs: u64) -> ObservedAt {
    ObservedAt::new(Duration::from_secs(elapsed_secs), Duration::from_secs(offset_secs))
}

fn connection_record(peer: Peer, conn_id: ConnectionId, at: ObservedAt, advertisable: bool) -> ConnectionRecord {
    ConnectionRecord {
        peer,
        conn_id,
        direction: ConnectionDirection::Outbound,
        full_duplex: false,
        advertisable,
        local_use: MiniProtocolUse::None,
        remote_initiators: RemoteInitiators::default(),
        established_at: at,
    }
}

fn conn_ids() -> (ConnectionId, ConnectionId) {
    let mut ids = ConnectionId::initial();
    let first = ids.get_and_increment();
    let second = ids.get_and_increment();
    (first, second)
}

#[test]
fn established_connection_is_stored_and_marks_ever_connected() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");
    let (id, _) = conn_ids();
    let established_at = observed(1, 0);
    let at = observed(2, 5);
    let mut record = connection_record(alice, id, established_at, false);
    record.full_duplex = true;
    record.direction = ConnectionDirection::Inbound;
    peers.record_connection_established(record.clone(), at);

    assert_eq!(peers.connection(id).cloned(), Some(record));
    assert_eq!(
        peers.share_flags(&alice),
        Some(PeerShareFlags { ever_connected: true, advertisable: false, failure_count: 0, adversarial: false })
    );
    let last = peers.scores(&alice).last_change.expect("observation time");
    assert_eq!(last.sim_elapsed(), Duration::from_secs(2));
    assert_eq!(last.duration_since_global_epoch(), Duration::from_secs(7));
}

#[test]
fn last_close_clears_claims_and_keeps_reputation() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");
    peers.record_intersection(alice, tip(1, 1), None, t(1));
    let at = observed(4, 9);
    let (id_a, id_b) = conn_ids();
    peers.record_connection_established(connection_record(alice, id_a, at, true), at);
    peers.record_connection_established(connection_record(alice, id_b, at, true), at);

    peers.record_connection_closed(alice, id_a, CloseReason::BearerEnded, at);
    let mid = peers.snapshot(&alice).expect("peer row");
    assert!(!mid.tips.is_empty(), "claims stay while another bearer is live");
    assert!(peers.connection(id_b).is_some());
    assert_eq!(peers.last_close(&alice), Some((id_a, CloseReason::BearerEnded, at)));

    let later = observed(5, 9);
    peers.record_connection_closed(alice, id_b, CloseReason::LocalDisconnect, later);
    let end = peers.snapshot(&alice).expect("reputation stub");
    assert!(end.tips.is_empty());
    assert!(end.share.ever_connected);
    assert!(end.share.advertisable);
    assert!(peers.connection(id_a).is_none());
    assert!(peers.connection(id_b).is_none());
    assert_eq!(peers.last_close(&alice), Some((id_b, CloseReason::LocalDisconnect, later)));
}

#[test]
fn close_for_unknown_connection_or_other_peer_changes_nothing() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");
    let bob = peer("bob");
    peers.record_intersection(alice, tip(1, 1), None, t(1));
    let at = observed(1, 0);
    let (id_a, id_b) = conn_ids();
    peers.record_connection_established(connection_record(alice, id_a, at, true), at);

    peers.record_connection_closed(alice, id_b, CloseReason::BearerEnded, at);
    peers.record_connection_closed(bob, id_a, CloseReason::LocalDisconnect, observed(3, 0));

    assert_eq!(peers.connection(id_a).map(|conn| conn.peer), Some(alice));
    assert!(!peers.snapshot(&alice).expect("row").tips.is_empty());
    assert!(peers.last_close(&alice).is_none());
    assert!(peers.last_close(&bob).is_none());
}

#[test]
fn local_use_applies_only_to_the_named_bearer() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");
    let bob = peer("bob");
    let at = observed(1, 2);
    let (id, missing) = conn_ids();
    peers.record_connection_established(connection_record(alice, id, at, true), at);

    let applied = observed(8, 2);
    assert!(peers.may_initiate().is_empty());
    peers.record_local_use_applied(alice, id, MiniProtocolUse::Diffusion, applied);
    assert_eq!(peers.connection(id).map(|conn| conn.local_use), Some(MiniProtocolUse::Diffusion));
    assert_eq!(peers.use_applied_at(id), Some(applied));
    assert_eq!(peers.may_initiate(), BTreeSet::from([id]));

    peers.record_local_use_applied(bob, id, MiniProtocolUse::Maintenance, observed(9, 2));
    assert_eq!(peers.connection(id).map(|conn| conn.local_use), Some(MiniProtocolUse::Diffusion));
    assert_eq!(peers.use_applied_at(id), Some(applied));

    peers.record_local_use_applied(alice, missing, MiniProtocolUse::None, observed(10, 2));
    assert!(peers.use_applied_at(missing).is_none());
}

#[test]
fn connect_failure_raises_malus_without_ever_connected() {
    let mut peers = PeerPerformance::new();
    let bob = peer("bob");
    let at = observed(3, 4);
    peers.record_connect_failed(bob, at);
    assert_eq!(
        peers.share_flags(&bob),
        Some(PeerShareFlags { ever_connected: false, advertisable: false, failure_count: 1, adversarial: false })
    );
    assert_eq!(peers.last_connect_failure(&bob), Some(at));
    let last = peers.scores(&bob).last_change.expect("failure time");
    assert_eq!(last.sim_elapsed(), Duration::from_secs(3));
    assert_eq!(last.duration_since_global_epoch(), Duration::from_secs(7));
    assert_eq!(peers.generation(), 1);
    assert!(peers.query_peer_view(0).is_some());
    assert!(peers.query_peer_view(1).is_none());

    peers.record_connection_failure(peer("carol"), t(4));
    assert_eq!(peers.generation(), 2);
    assert!(peers.query_peer_view(1).is_some(), "a failure write moves the view generation");
}

#[test]
fn keepalive_sample_keeps_both_clock_fields() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");
    let at = observed(11, 70);
    let generation = peers.generation();
    peers.record_keepalive_sample(alice, Duration::from_millis(12), at);
    assert_eq!(peers.scores(&alice).keepalive_rtt_latest, Some(Duration::from_millis(12)));
    assert_eq!(peers.scores(&alice).keepalive_rtt_ewma, Some(Duration::from_millis(12)));
    let last = peers.scores(&alice).last_change.expect("sample time");
    assert_eq!(last.sim_elapsed(), Duration::from_secs(11));
    assert_eq!(last.duration_since_global_epoch(), Duration::from_secs(81));
    assert_eq!(peers.generation(), generation);
}

#[test]
fn keepalive_rtt_summary_keeps_latest_and_smoothed() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");
    let first = Duration::from_millis(12);
    let second = Duration::from_millis(32);
    peers.record_keepalive_rtt(alice, first, t(1));
    peers.record_keepalive_rtt(alice, second, t(2));

    let expected = Duration::from_secs_f64((1.0 - 0.2) * first.as_secs_f64() + 0.2 * second.as_secs_f64());
    let scores = peers.scores(&alice);
    assert_eq!(scores.keepalive_rtt_latest, Some(second));
    assert_eq!(scores.keepalive_rtt_ewma, Some(expected));
    assert_eq!(peers.snapshot(&alice).expect("row").scores, scores);
    assert_eq!(peers.churn_inputs(&[alice])[0].scores, scores);
}

#[test]
fn shared_peers_and_share_requests_are_recorded() {
    let mut peers = PeerPerformance::new();
    let donor = Peer::for_test(4001);
    let other = Peer::for_test(4002);
    let asker = Peer::for_test(4003);
    let addr = SocketAddr::from(other);
    let at = observed(2, 0);
    peers.record_shared_peers(&donor, &[addr], at);
    assert!(peers.shared_contains(&other));
    assert_eq!(peers.last_shared_at(&donor), Some(at));
    assert_eq!(peers.query_share_peers(&asker, 1, at), vec![addr]);
    assert!(peers.query_share_peers(&asker, 0, at).is_empty());

    peers.record_share_request_served(asker, 3, observed(1, 2));
    peers.record_share_request_served(asker, 5, observed(6, 2));
    assert_eq!(
        peers.share_requests(&asker),
        Some(ShareRequestRecord {
            count: 2,
            last_amount: 5,
            last_at: observed(6, 2),
            window_start: observed(1, 2),
            current_window: 2,
            previous_window: 0,
        })
    );
}

#[test]
fn share_request_window_rolls_current_into_previous() {
    let mut peers = PeerPerformance::new();
    let asker = Peer::for_test(4003);
    let start = observed(100, 2);
    let at = |extra: Duration| ObservedAt::new(start.elapsed + extra, start.global_epoch_offset);

    peers.record_share_request_served(asker, 3, start);
    peers.record_share_request_served(asker, 4, at(SHARE_REQUEST_WINDOW - Duration::from_secs(1)));
    let inside = peers.share_requests(&asker).expect("row");
    assert_eq!(inside.count, 2);
    assert_eq!(inside.last_amount, 4);
    assert_eq!(inside.current_window, 2);
    assert_eq!(inside.previous_window, 0);
    assert_eq!(inside.window_start, start);

    peers.record_share_request_served(asker, 5, at(SHARE_REQUEST_WINDOW));
    let rolled = peers.share_requests(&asker).expect("row");
    assert_eq!(rolled.count, 3);
    assert_eq!(rolled.last_amount, 5);
    assert_eq!(rolled.last_at, at(SHARE_REQUEST_WINDOW));
    assert_eq!(rolled.current_window, 1);
    assert_eq!(rolled.previous_window, 2);
    assert_eq!(rolled.window_start, at(SHARE_REQUEST_WINDOW));

    peers.record_share_request_served(asker, 1, at(SHARE_REQUEST_WINDOW + Duration::from_secs(10)));
    peers.record_share_request_served(asker, 7, at(SHARE_REQUEST_WINDOW.saturating_mul(3)));
    let skipped = peers.share_requests(&asker).expect("row");
    assert_eq!(skipped.count, 5);
    assert_eq!(skipped.last_amount, 7);
    assert_eq!(skipped.current_window, 1);
    assert_eq!(skipped.previous_window, 0);
    assert_eq!(skipped.window_start, at(SHARE_REQUEST_WINDOW.saturating_mul(3)));
    assert_eq!(peers.generation(), 0);
}

#[test]
fn share_request_window_stays_on_the_boundary_when_the_sample_is_off_grid() {
    let mut peers = PeerPerformance::new();
    let asker = Peer::for_test(4004);
    let start = observed(100, 2);
    let at = |extra: Duration| ObservedAt::new(start.elapsed + extra, start.global_epoch_offset);

    peers.record_share_request_served(asker, 3, start);
    peers.record_share_request_served(asker, 4, at(Duration::from_secs(90)));
    let rolled = peers.share_requests(&asker).expect("row");
    assert_eq!(rolled.count, 2);
    assert_eq!(rolled.current_window, 1);
    assert_eq!(rolled.previous_window, 1, "one whole window rolls the current count forward");
    assert_eq!(rolled.window_start, at(SHARE_REQUEST_WINDOW), "the window stays on the 60s boundary");
    assert_eq!(peers.generation(), 0);
}

#[test]
fn observations_do_not_change_fetch_or_share_replies() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");
    let bob = peer("bob");
    peers.record_header_announcement(alice, tip(1, 1), None, t(1));
    peers.record_header_announcement(bob, tip(1, 1), None, t(1));
    let fetch_before = peers.select_peers_for_fetch(select(vec![hash(1)], 2));
    assert_eq!(fetch_before.peers.len(), 2);
    // The peer fetch currently prefers gets the slow sample. Reading RTT would reorder.
    peers.record_keepalive_rtt(fetch_before.peers[0], Duration::from_secs(3600), t(3));
    peers.record_keepalive_rtt(fetch_before.peers[1], Duration::from_millis(1), t(3));
    assert_eq!(peers.select_peers_for_fetch(select(vec![hash(1)], 2)), fetch_before);

    let mut churn_peers = PeerPerformance::new();
    churn_peers.record_header_announcement(alice, tip(1, 1), None, t(1));
    churn_peers.record_header_announcement(bob, tip(1, 1), None, t(1));
    let churn_before: Vec<_> =
        churn_peers.rank_peers_for_churn(&[alice, bob], t(2)).into_iter().map(|row| row.peer).collect();
    assert_eq!(churn_before.len(), 2);
    // Worst-first: the last peer is the one churn prefers. A slow sample there would move it first.
    churn_peers.record_keepalive_rtt(churn_before[1], Duration::from_secs(3600), t(3));
    churn_peers.record_keepalive_rtt(churn_before[0], Duration::from_millis(1), t(3));
    let churn_after: Vec<_> =
        churn_peers.rank_peers_for_churn(&[alice, bob], t(4)).into_iter().map(|row| row.peer).collect();
    assert_eq!(churn_after, churn_before);

    let donor = Peer::for_test(4001);
    let other = Peer::for_test(4002);
    let asker = Peer::for_test(4003);
    let at = observed(2, 0);
    peers.record_shared_peers(&donor, &[SocketAddr::from(other), SocketAddr::from(donor)], at);
    let now = t(2);
    let reply_before = peers.select_share_peers(&asker, 10, now);
    assert!(!reply_before.is_empty());
    for step in 0..5 {
        let when =
            ObservedAt::new(at.elapsed + SHARE_REQUEST_WINDOW.saturating_mul(step as u32), at.global_epoch_offset);
        peers.record_share_request_served(asker, 10, when);
        peers.record_share_request_served(other, 10, when);
    }
    assert_eq!(peers.select_share_peers(&asker, 10, now), reply_before);
}

#[test]
fn adversarial_mark_clears_rtt_and_share_rate_and_close_keeps_them() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");
    let (id, _) = conn_ids();
    let at = observed(1, 2);
    peers.record_connection_established(connection_record(alice, id, at, true), at);
    peers.record_keepalive_sample(alice, Duration::from_millis(12), at);
    peers.record_share_request_served(alice, 3, at);

    peers.record_connection_closed(alice, id, CloseReason::BearerEnded, observed(2, 2));
    assert_eq!(peers.scores(&alice).keepalive_rtt_latest, Some(Duration::from_millis(12)));
    assert!(peers.share_requests(&alice).is_some());

    peers.mark_adversarial(&alice, t(4));
    assert!(peers.scores(&alice).keepalive_rtt_latest.is_none());
    assert!(peers.scores(&alice).keepalive_rtt_ewma.is_none());
    assert!(peers.share_requests(&alice).is_none());
}

#[test]
fn install_shares_one_worker_and_a_later_query_sees_the_write() {
    let resources = Resources::default();
    let _join = Performance::new().install(&resources);
    let performance = resources.get::<ResourcePerformance>().expect("performance").clone();
    let tracking = resources.get::<PeerTrackingResource>().expect("peer tracking").clone();
    assert_eq!(Arc::as_ptr(&performance) as *const (), Arc::as_ptr(&tracking) as *const ());

    let alice = peer("alice");
    let at = observed(4, 1);
    let (id, _) = conn_ids();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
    rt.block_on(tracking.record_connection_established(connection_record(alice, id, at, true), at));

    let donor = Peer::for_test(4101);
    let other = Peer::for_test(4102);
    let asker = Peer::for_test(4103);
    rt.block_on(tracking.record_shared_peers(donor, vec![SocketAddr::from(other)], at));
    let flags_response = rt.block_on(Box::new(Performance::share_flags(alice)).run(resources));
    let flags = *flags_response.cast::<Option<PeerShareFlags>>().expect("share flags");
    assert_eq!(
        flags,
        Some(PeerShareFlags { ever_connected: true, advertisable: true, failure_count: 0, adversarial: false })
    );
    let shared = rt.block_on(tracking.query_share_peers(asker, 1, at));
    assert_eq!(shared, vec![SocketAddr::from(other)]);
}

#[test]
fn lifecycle_writes_bump_generation() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");
    let at = observed(1, 0);
    let (id, _) = conn_ids();
    assert_eq!(peers.generation(), 0);
    assert!(peers.query_peer_view(0).is_none());

    peers.record_connection_established(connection_record(alice, id, at, true), at);
    assert_eq!(peers.generation(), 1);
    assert!(peers.query_peer_view(0).is_some());
    assert!(peers.query_peer_view(1).is_none());

    peers.record_local_use_applied(alice, id, MiniProtocolUse::Diffusion, observed(2, 0));
    assert_eq!(peers.generation(), 2);
    peers.record_connect_failed(peer("bob"), observed(3, 0));
    assert_eq!(peers.generation(), 3);
    peers.record_connection_closed(alice, id, CloseReason::BearerEnded, observed(4, 0));
    assert_eq!(peers.generation(), 4);

    peers.record_connection_closed(alice, id, CloseReason::BearerEnded, observed(5, 0));
    assert_eq!(peers.generation(), 4, "a close for an unknown bearer is not a write");
}

#[test]
fn remote_initiator_bits_derive_temperature_and_move_generation_only_on_change() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");
    let bob = peer("bob");
    let at = observed(1, 0);
    let (id, other) = conn_ids();

    assert!(peers.record_remote_use(alice, id, RemoteProtocol::ChainSync, true, at).is_none());
    assert_eq!(peers.generation(), 0);

    peers.record_connection_established(connection_record(alice, id, at, true), at);
    let established = peers.generation();
    assert!(peers.record_remote_use(bob, id, RemoteProtocol::KeepAlive, true, at).is_none());
    assert_eq!(peers.generation(), established);

    assert_eq!(
        peers.record_remote_use(alice, id, RemoteProtocol::ChainSync, true, observed(2, 0)),
        Some(MiniProtocolUse::Diffusion),
    );
    assert_eq!(peers.record_remote_use(alice, id, RemoteProtocol::BlockFetch, true, observed(3, 0)), None,);
    assert_eq!(peers.record_remote_use(alice, id, RemoteProtocol::TxSubmission, true, observed(4, 0)), None,);
    let hot = peers.generation();
    assert_eq!(hot, established + 1, "a second hot protocol does not move the generation");

    assert!(peers.record_remote_use(alice, id, RemoteProtocol::KeepAlive, true, observed(5, 0)).is_none());
    assert_eq!(peers.generation(), hot);
    assert!(peers.record_remote_use(alice, id, RemoteProtocol::KeepAlive, true, observed(6, 0)).is_none());
    assert_eq!(peers.generation(), hot);

    assert_eq!(peers.record_remote_use(alice, id, RemoteProtocol::ChainSync, false, observed(7, 0)), None,);
    assert_eq!(peers.record_remote_use(alice, id, RemoteProtocol::BlockFetch, false, observed(8, 0)), None,);
    assert_eq!(
        peers.record_remote_use(alice, id, RemoteProtocol::TxSubmission, false, observed(9, 0)),
        Some(MiniProtocolUse::Maintenance),
    );
    assert_eq!(
        peers.record_remote_use(alice, id, RemoteProtocol::KeepAlive, false, observed(10, 0)),
        Some(MiniProtocolUse::None),
    );

    peers.record_remote_use(alice, id, RemoteProtocol::PeerSharing, true, observed(11, 0));
    let view = peers.query_peer_view(0).expect("writes moved the generation");
    let conn = view.connections.iter().find(|conn| conn.conn_id == id).expect("bearer");
    assert_eq!(conn.remote_use, MiniProtocolUse::Maintenance);
    assert!(conn.remote_initiators.is_active(RemoteProtocol::PeerSharing));
    assert!(!conn.remote_initiators.is_active(RemoteProtocol::ChainSync));

    let generation = peers.generation();
    assert!(peers.record_remote_use(alice, other, RemoteProtocol::KeepAlive, true, observed(12, 0)).is_none());
    assert_eq!(peers.generation(), generation);
}

#[test]
fn uninteresting_marks_are_the_latest_per_peer_and_only_those_since_the_query() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");
    let bob = peer("bob");
    let (alice_id, bob_id) = conn_ids();
    let at = observed(1, 0);
    peers.record_connection_established(connection_record(alice, alice_id, at, false), at);
    peers.record_connection_established(connection_record(bob, bob_id, at, false), at);

    peers.record_uninteresting(alice, alice_id, false, at);
    let first = peers.generation();
    let view = peers.query_peer_view(0).expect("generation moved");
    assert_eq!(view.uninteresting.len(), 1);
    assert_eq!(view.uninteresting[0].peer, alice);
    assert!(!view.uninteresting[0].after_rollback);

    peers.record_uninteresting(alice, alice_id, true, observed(2, 0));
    let view = peers.query_peer_view(first).expect("repeated mark moves the generation");
    assert_eq!(view.uninteresting.len(), 1);
    assert!(view.uninteresting[0].after_rollback, "the latest mark replaces the earlier one");

    let seen = peers.generation();
    peers.set_ledger_candidates(Default::default());
    let view = peers.query_peer_view(seen).expect("ledger replacement moves the generation");
    assert!(view.uninteresting.is_empty(), "an older mark is not delivered again");

    peers.record_uninteresting(bob, bob_id, false, observed(3, 0));
    peers.record_connection_closed(bob, bob_id, CloseReason::BearerEnded, observed(4, 0));
    let view = peers.query_peer_view(seen).expect("close moves the generation");
    assert!(view.uninteresting.iter().all(|mark| mark.peer != bob), "a closed bearer drops its mark");
    assert!(view.uninteresting.iter().all(|mark| mark.peer != alice));
}

#[test]
fn a_mark_without_a_live_bearer_is_not_stored() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");
    let (alice_id, other_id) = conn_ids();
    let at = observed(1, 0);

    peers.record_uninteresting(alice, alice_id, false, at);
    assert_eq!(peers.generation(), 0);
    assert!(peers.query_peer_view(0).is_none(), "nothing was stored");

    peers.record_connection_established(connection_record(alice, alice_id, at, false), at);
    let generation = peers.generation();
    peers.record_uninteresting(alice, other_id, true, observed(2, 0));
    assert_eq!(peers.generation(), generation, "a different connection is not this bearer");
    let view = peers.query_peer_view(0).expect("the establish moved the generation");
    assert!(view.uninteresting.is_empty());
}

#[test]
fn select_outbound_is_stable_for_a_fixed_seed() {
    use std::collections::BTreeSet;

    use amaru_kernel::PeerCandidate;

    use super::{PeerMix, SelectOutboundParams};

    let left: Peer = "10.0.0.1:4001".parse().expect("peer");
    let right: Peer = "10.0.0.2:4002".parse().expect("peer");
    let peers = PeerPerformance::with_sources(
        BTreeSet::from([PeerCandidate::from(left), PeerCandidate::from(right)]),
        BTreeSet::new(),
        BTreeSet::new(),
        PeerMix::default(),
    );
    let params =
        SelectOutboundParams { open: 1, excluded: BTreeSet::new(), eligible_inbound: 0, seed: [0x42; 32], now: t(1) };
    let first = peers.select_outbound(params.clone());
    let second = peers.select_outbound(params);
    assert_eq!(first, second);
    assert_eq!(first.outbound.len(), 1, "{first:?}");
    assert_eq!(first.outbound[0].candidate, PeerCandidate::from(right), "{first:?}");
}

/// The simulation clock does not advance while the performance worker runs, so a world
/// test cannot see how long `record_header_announcement` and `select_peers_for_fetch`
/// waited behind a burst of connection closes. This test pauses the worker, queues one
/// eviction batch against a cap-sized map, then 32 establish/close pairs, then those two
/// ops, and checks that the urgent ops waited to be dequeued.
#[test]
fn instrumented_worker_queue_wait() {
    use std::collections::BTreeSet;

    use tokio::sync::oneshot;

    use super::ops::{PeerOp, PerformanceOp};
    use crate::performance::{
        EVICTION_BATCH, EvictRecordsEffect, RecordHeaderAnnouncementEffect, SelectPeersForFetchEffect,
    };

    let now = t(200_000);
    let stale = now - Duration::from_secs(48 * 60 * 60);
    let mut seeded = PeerPerformance::new();
    seeded.testing_seed_eviction_cap(stale, now);
    let direct = {
        let (protected_peers, protected_candidates) = (BTreeSet::new(), BTreeSet::new());
        seeded.evict_batch(now, &protected_peers, &protected_candidates)
    };
    assert_eq!(direct.examined, EVICTION_BATCH);
    assert!(direct.removed_records > 0 && direct.removed_shared > 0, "{direct:?}");

    let mut queued = PeerPerformance::new();
    queued.testing_seed_eviction_cap(stale, now);
    let (perf, release, probe) = Performance::paused_with(queued);
    perf.submit(PerformanceOp::Peer(PeerOp::EvictRecords {
        effect: EvictRecordsEffect { now, protected_peers: BTreeSet::new(), protected_candidates: BTreeSet::new() },
    }));
    let at = observed(1, 0);
    let mut ids = ConnectionId::initial();
    for n in 0..32u16 {
        let id = ids.get_and_increment();
        let who = Peer::for_test(5_000 + n);
        let (established_tx, _established_rx) = oneshot::channel();
        perf.submit(PerformanceOp::Peer(PeerOp::RecordConnectionEstablished {
            conn: connection_record(who, id, at, false),
            at,
            reply: established_tx,
        }));
        perf.submit(PerformanceOp::Peer(PeerOp::RecordConnectionClosed {
            peer: who,
            conn_id: id,
            reason: CloseReason::BearerEnded,
            at,
        }));
    }
    let announcer = peer("announcer");
    let (header_tx, header_rx) = oneshot::channel();
    perf.submit(PerformanceOp::Peer(PeerOp::RecordHeaderAnnouncement {
        effect: RecordHeaderAnnouncementEffect {
            peer: announcer,
            header: tip(1, 1),
            parent: None,
            at: t(2),
            slot_start_to_header_micros: 0,
            slot_onset: Duration::ZERO,
            already_stored: false,
        },
        reply: header_tx,
    }));
    let (fetch_tx, fetch_rx) = oneshot::channel();
    perf.submit(PerformanceOp::Peer(PeerOp::SelectPeersForFetch {
        effect: SelectPeersForFetchEffect { params: select(vec![hash(1)], 1) },
        reply: fetch_tx,
    }));
    release.send(()).expect("worker is waiting");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
    rt.block_on(async {
        header_rx.await.expect("header announcement");
        fetch_rx.await.expect("fetch selection");
    });
    let waited = probe.max_urgent_wait();
    assert!(!waited.is_zero(), "urgent ops were not timed");
}

#[test]
fn a_remote_share_request_keeps_the_same_addresses() {
    use std::collections::BTreeSet;

    use amaru_kernel::PeerCandidate;

    use crate::performance::PeerMix;

    let requester = Peer::for_test(5000);
    let failed = Peer::for_test(5012);
    let ledger = Peer::for_test(5013);
    let mut static_peers = BTreeSet::from([PeerCandidate::from(requester), PeerCandidate::from(failed)]);
    for port in 5001..=5011 {
        static_peers.insert(PeerCandidate::from(Peer::for_test(port)));
    }
    let ledger_candidates = BTreeSet::from([PeerCandidate::from(ledger)]);
    let mix = PeerMix::default();
    let at = observed(2, 0);
    let now = t(2);

    let mut direct =
        PeerPerformance::with_sources(static_peers.clone(), BTreeSet::new(), ledger_candidates.clone(), mix.clone());
    direct.record_connect_failed(failed, at);
    let before = direct.select_share_peers(&requester, 20, now);

    let resources = Resources::default();
    let _join =
        Performance::with_peer_sources(static_peers, BTreeSet::new(), ledger_candidates, mix).install(&resources);
    let tracking = resources.get::<PeerTrackingResource>().expect("peer tracking").clone();
    tracking.record_connect_failed(failed, at);
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
    let after = rt.block_on(tracking.query_share_peers(requester, 20, at));
    let again = rt.block_on(tracking.query_share_peers(requester, 20, at));

    assert_eq!(after, before, "the resource reply diverged from the previous share sample");
    assert_eq!(again, after);
    assert_eq!(after.len(), 10);
    assert!(!after.iter().any(|addr| {
        *addr == SocketAddr::from(requester) || *addr == SocketAddr::from(failed) || *addr == SocketAddr::from(ledger)
    }));
    assert_eq!(
        after,
        vec![
            SocketAddr::from(Peer::for_test(5003)),
            SocketAddr::from(Peer::for_test(5001)),
            SocketAddr::from(Peer::for_test(5006)),
            SocketAddr::from(Peer::for_test(5005)),
            SocketAddr::from(Peer::for_test(5002)),
            SocketAddr::from(Peer::for_test(5010)),
            SocketAddr::from(Peer::for_test(5009)),
            SocketAddr::from(Peer::for_test(5011)),
            SocketAddr::from(Peer::for_test(5004)),
            SocketAddr::from(Peer::for_test(5007)),
        ],
    );
}
