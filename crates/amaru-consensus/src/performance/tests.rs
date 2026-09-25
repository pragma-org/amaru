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
    io::{self, Write},
    sync::{Arc, Mutex},
    time::Duration,
};

use amaru_kernel::{BlockHeight, HeaderHash, Peer, Point, Slot};
use amaru_observability::{CborConsoleEventFormat, console_field_formatter, debug, info};
use amaru_pure_stage::{ExternalEffect, Instant, Resources};
use tracing_subscriber::{EnvFilter, prelude::*};

use super::{
    ClaimKind, HeaderLifecycleOutcome, HeaderPerformance, PeerPerformance, PeerShareFlags, Performance,
    ResourcePerformance, SelectPeersParams,
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
    SelectPeersParams { need, max_peers, now: t(100) }
}

// ---------------------------------------------------------------------------
// Peer coverage / scores (local PeerPerformance)
// ---------------------------------------------------------------------------

#[test]
fn intersect_only_covers_need_ending_at_intersect_not_unknown_child() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");
    let h1 = tip(1, 1);

    peers.apply_intersection(alice, h1, None, t(1));

    assert!(peers.apply_peer_covers_fragment(&alice, &[hash(1)]));
    assert!(!peers.apply_peer_covers_fragment(&alice, &[hash(1), hash(2)]));
    assert!(!peers.apply_peer_covers_fragment(&alice, &[hash(2)]));
}

#[test]
fn announce_chain_collapses_to_single_tip_and_covers_ancestors() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");

    peers.apply_header_announcement(alice, tip(1, 1), None, t(1));
    peers.apply_header_announcement(alice, tip(2, 2), Some(hash(1)), t(2));

    let snap = peers.apply_snapshot(&alice).expect("alice present");
    assert_eq!(snap.tips.len(), 1);
    assert_eq!(snap.tips[0].hash, hash(2));

    assert!(peers.apply_peer_covers_fragment(&alice, &[hash(1)]));
    assert!(peers.apply_peer_covers_fragment(&alice, &[hash(1), hash(2)]));
    assert!(peers.apply_peer_covers_fragment(&alice, &[hash(2)]));
}

#[test]
fn duplicate_announcers_both_selected() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");
    let bob = peer("bob");

    peers.apply_header_announcement(alice, tip(1, 1), None, t(1));
    peers.apply_header_announcement(bob, tip(1, 1), None, t(2));

    let set = peers.apply_select_peers_for_fetch(select(vec![hash(1)], 2));
    assert!(!set.weak);
    assert_eq!(set.peers.len(), 2);
    assert!(set.peers.contains(&alice));
    assert!(set.peers.contains(&bob));

    let first = peers.apply_first_announced_at(&hash(1)).expect("first announcer");
    assert_eq!(first.0, alice);
    assert_eq!(first.1, t(1));
}

#[test]
fn descendant_claim_covers_ancestor_via_parent_walk() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");

    peers.apply_header_announcement(alice, tip(1, 1), None, t(1));
    peers.apply_header_announcement(alice, tip(2, 2), Some(hash(1)), t(2));
    peers.apply_header_announcement(alice, tip(3, 3), Some(hash(2)), t(3));

    assert!(peers.apply_peer_covers_fragment(&alice, &[hash(1)]));
    assert!(peers.apply_peer_covers_fragment(&alice, &[hash(2)]));
    assert!(peers.apply_peer_covers_fragment(&alice, &[hash(1), hash(2), hash(3)]));
}

#[test]
fn parent_walk_stops_at_target_height_on_wrong_branch() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");
    let bob = peer("bob");

    for h in 1u8..=5 {
        let parent = (h > 1).then(|| hash(h - 1));
        peers.apply_header_announcement(alice, tip(h, h as u64), parent, t(h as u64));
    }
    peers.apply_intersection(bob, tip(30, 3), Some(hash(2)), t(10));

    assert!(!peers.apply_peer_covers_fragment(&alice, &[hash(30)]));
    assert!(peers.apply_peer_covers_fragment(&alice, &[hash(5)]));
    assert!(peers.apply_peer_covers_fragment(&alice, &[hash(2)]));
}

#[test]
fn ancestor_only_claim_does_not_cover_descendant() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");

    peers.apply_intersection(alice, tip(1, 1), None, t(1));

    assert!(peers.apply_peer_covers_fragment(&alice, &[hash(1)]));
    assert!(!peers.apply_peer_covers_fragment(&alice, &[hash(2)]));
    assert!(!peers.apply_peer_covers_fragment(&alice, &[hash(1), hash(2)]));
}

#[test]
fn intersect_at_tip_of_need_covers_full_fragment_via_index() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");

    peers.apply_intersection(alice, tip(3, 3), Some(hash(2)), t(1));

    assert!(peers.apply_peer_covers_fragment(&alice, &[hash(1), hash(2), hash(3)]));
    assert!(peers.apply_peer_covers_fragment(&alice, &[hash(3)]));
}

#[test]
fn rollback_drops_fork_tip_and_restores_point() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");

    peers.apply_header_announcement(alice, tip(1, 1), None, t(1));
    peers.apply_header_announcement(alice, tip(2, 2), Some(hash(1)), t(2));
    peers.apply_header_announcement(alice, tip(10, 3), Some(hash(1)), t(3));

    let before = peers.apply_snapshot(&alice).expect("alice");
    assert!(!before.tips.is_empty());

    peers.apply_rollback(alice, tip(1, 1), None, t(4));

    let after = peers.apply_snapshot(&alice).expect("alice");
    assert!(after.tips.iter().any(|c| c.hash == hash(1)));
    assert!(!after.tips.iter().any(|c| c.hash == hash(2)));
    assert!(!after.tips.iter().any(|c| c.hash == hash(10)));
    assert!(peers.apply_peer_covers_fragment(&alice, &[hash(1)]));
    assert!(!peers.apply_peer_covers_fragment(&alice, &[hash(2)]));
}

#[test]
fn prune_removes_old_tips_but_retains_scores() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");

    peers.apply_header_announcement(alice, tip(1, 1), None, t(1));
    peers.apply_block_delivery(alice, hash(1), BlockHeight::from(1), None, t(2), Duration::from_millis(50), 90_000);

    let scores_before = peers.apply_scores(&alice);
    assert!(scores_before.block_response_ewma.is_some());
    assert_eq!(scores_before.fetch_successes, 1);

    peers.apply_prune_below(BlockHeight::from(5));

    let snap = peers.apply_snapshot(&alice).expect("alice kept for scores");
    assert!(snap.tips.is_empty());
    assert!(!peers.apply_peer_covers_fragment(&alice, &[hash(1)]));
    assert_eq!(peers.apply_scores(&alice).fetch_successes, 1);
    assert!(peers.apply_scores(&alice).block_response_ewma.is_some());
}

#[test]
fn clear_availability_keeps_scores_and_share_flags() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");

    peers.apply_advertisability(alice, true, t(0));
    peers.apply_header_announcement(alice, tip(1, 1), None, t(1));
    peers.apply_block_delivery(alice, hash(1), BlockHeight::from(1), None, t(2), Duration::from_millis(20), 1000);

    peers.apply_clear_peer_availability(&alice);
    assert!(!peers.apply_peer_covers_fragment(&alice, &[hash(1)]));
    assert_eq!(peers.apply_scores(&alice).fetch_successes, 1);
    assert!(peers.apply_direct_claimants(&hash(1)).is_empty());
    assert_eq!(
        peers.apply_share_flags(&alice),
        Some(PeerShareFlags { ever_connected: true, advertisable: true, failure_count: 0, adversarial: false })
    );
    assert!(peers.apply_ok_for_sharing(&alice, t(10)));
}

#[test]
fn peer_adversarial_keeps_reputation_stub_clears_claims_and_scores() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");

    peers.apply_advertisability(alice, true, t(0));
    peers.apply_connection_failure(alice, t(1));
    peers.apply_header_announcement(alice, tip(1, 1), None, t(2));
    peers.apply_block_delivery(alice, hash(1), BlockHeight::from(1), None, t(3), Duration::from_millis(20), 1000);

    peers.apply_peer_adversarial(&alice, t(4));

    let snap = peers.apply_snapshot(&alice).expect("stub retained after adversarial mark");
    assert!(snap.tips.is_empty());
    assert_eq!(snap.scores.fetch_successes, 0);
    assert!(snap.scores.block_response_ewma.is_none());
    assert_eq!(
        snap.share,
        PeerShareFlags { ever_connected: true, advertisable: true, failure_count: 1, adversarial: true }
    );
    assert!(!peers.apply_ok_for_sharing(&alice, t(10)));
    assert!(peers.apply_direct_claimants(&hash(1)).is_empty());
}

#[test]
fn advertisability_latest_handshake_wins() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");

    peers.apply_advertisability(alice, true, t(1));
    assert!(peers.apply_ok_for_sharing(&alice, t(10)));

    peers.apply_advertisability(alice, false, t(2));
    assert_eq!(
        peers.apply_share_flags(&alice),
        Some(PeerShareFlags { ever_connected: true, advertisable: false, failure_count: 0, adversarial: false })
    );
    assert!(!peers.apply_ok_for_sharing(&alice, t(10)));

    peers.apply_advertisability(alice, true, t(3));
    assert!(peers.apply_ok_for_sharing(&alice, t(10)));
}

#[test]
fn connection_failure_blocks_sharing_until_malus_decays() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");

    assert!(!peers.apply_ok_for_sharing(&alice, t(10)));

    peers.apply_advertisability(alice, true, t(1));
    assert!(peers.apply_ok_for_sharing(&alice, t(10)));

    peers.apply_connection_failure(alice, t(2));
    assert_eq!(peers.apply_share_flags(&alice).map(|f| f.failure_count), Some(1));
    assert!(!peers.apply_ok_for_sharing(&alice, t(2)));

    peers.apply_connection_failure(alice, t(3));
    assert_eq!(peers.apply_share_flags(&alice).map(|f| f.failure_count), Some(2));
    // Still high shortly after failures.
    assert!(!peers.apply_ok_for_sharing(&alice, t(3)));
}

#[test]
fn connection_failure_only_does_not_mark_ever_connected() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");

    peers.apply_connection_failure(alice, t(1));

    assert_eq!(
        peers.apply_share_flags(&alice),
        Some(PeerShareFlags { ever_connected: false, advertisable: false, failure_count: 1, adversarial: false })
    );
    assert!(!peers.apply_ok_for_sharing(&alice, t(10)));
}

#[test]
fn connection_malus_decays_with_half_life_without_new_samples() {
    use crate::performance::{CONNECT_FAIL_IMPULSE, DEFAULT_PEER_MALUS_HALF_LIFE, malus_at};

    let mut peers = PeerPerformance::new();
    let alice = peer("alice");
    peers.apply_connection_failure(alice, t(0));

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
    peers.apply_connection_failure(bad, t(1));

    // Open=1: should strongly prefer never-connected good over failed bad.
    let mut good_picks = 0;
    for i in 0..20u8 {
        let seed = [i; 32];
        let picked = peers.apply_select_outbound(SelectOutboundParams {
            open: 1,
            excluded: BTreeSet::new(),
            eligible_inbound: 0,
            seed,
            now: t(1),
        });
        if picked.outbound.iter().map(|p| p.candidate.as_peer()).collect::<Vec<_>>() == vec![Some(good)] {
            good_picks += 1;
        }
    }
    assert!(good_picks >= 15, "good_picks={good_picks}");
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
    let picked = peers.apply_select_outbound(SelectOutboundParams {
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
    let picked = peers.apply_select_outbound(SelectOutboundParams {
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
    peers.apply_note_dial(PeerSource::Static, &host, resolved);
    assert!(peers.apply_is_static_peer(&resolved));
    let picked = peers.apply_select_outbound(SelectOutboundParams {
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
    let picked = peers.apply_select_outbound(SelectOutboundParams {
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

    peers.apply_header_announcement(fast, tip(1, 1), None, t(1));
    peers.apply_header_announcement(fast, tip(2, 2), Some(hash(1)), t(2));
    peers.apply_block_delivery(
        fast,
        hash(2),
        BlockHeight::from(2),
        Some(hash(1)),
        t(3),
        Duration::from_millis(10),
        90_000,
    );

    peers.apply_header_announcement(slow, tip(1, 1), None, t(1));
    peers.apply_header_announcement(slow, tip(2, 2), Some(hash(1)), t(2));
    peers.apply_block_delivery(
        slow,
        hash(2),
        BlockHeight::from(2),
        Some(hash(1)),
        t(4),
        Duration::from_secs(2),
        90_000,
    );

    peers.apply_header_announcement(partial, tip(1, 1), None, t(1));

    let set = peers.apply_select_peers_for_fetch(select(vec![hash(1), hash(2)], 3));
    assert!(!set.weak);
    assert_eq!(set.peers, vec![fast, slow]);
    assert!(!set.peers.contains(&partial));
}

#[test]
fn prefix_only_peer_not_selected_for_range() {
    let mut peers = PeerPerformance::new();
    let prefix = peer("prefix");
    let full = peer("full");

    peers.apply_header_announcement(prefix, tip(1, 1), None, t(1));
    peers.apply_header_announcement(full, tip(1, 1), None, t(1));
    peers.apply_header_announcement(full, tip(2, 2), Some(hash(1)), t(2));

    let set = peers.apply_select_peers_for_fetch(select(vec![hash(1), hash(2)], 5));
    assert_eq!(set.peers, vec![full]);
    assert!(!set.peers.contains(&prefix));
}

#[test]
fn cold_start_empty_map_returns_weak_empty_selection() {
    let peers = PeerPerformance::new();
    let set = peers.apply_select_peers_for_fetch(select(vec![hash(1)], 5));
    assert!(set.weak);
    assert!(set.peers.is_empty());
}

#[test]
fn after_intersect_selection_becomes_non_empty() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");

    let empty = peers.apply_select_peers_for_fetch(select(vec![hash(5)], 5));
    assert!(empty.weak);

    peers.apply_intersection(alice, tip(5, 5), Some(hash(4)), t(1));

    let set = peers.apply_select_peers_for_fetch(select(vec![hash(5)], 5));
    assert!(!set.weak);
    assert_eq!(set.peers, vec![alice]);
}

#[test]
fn max_peers_bounds_selection() {
    let mut peers = PeerPerformance::new();
    for i in 0..10u8 {
        let p = peer(&format!("p{i}"));
        peers.apply_header_announcement(p, tip(1, 1), None, t(1));
    }
    let set = peers.apply_select_peers_for_fetch(select(vec![hash(1)], 3));
    assert_eq!(set.peers.len(), 3);
    assert!(!set.weak);
}

#[test]
fn churn_ranks_unreliable_peers_first() {
    let mut peers = PeerPerformance::new();
    let good = peer("good");
    let bad = peer("bad");

    peers.apply_header_announcement(good, tip(1, 1), None, t(1));
    peers.apply_block_delivery(good, hash(1), BlockHeight::from(1), None, t(2), Duration::from_millis(10), 1000);

    peers.apply_header_announcement(bad, tip(1, 1), None, t(1));
    peers.apply_fetch_failure(std::slice::from_ref(&bad), t(3));
    peers.apply_fetch_failure(std::slice::from_ref(&bad), t(4));

    let ranked = peers.apply_rank_peers_for_churn(&[good, bad], t(5));
    assert_eq!(ranked[0].0, bad);
    assert_eq!(ranked[1].0, good);
}

#[test]
fn claim_kind_strength_prefers_delivery_over_intersection() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");

    peers.apply_intersection(alice, tip(1, 1), None, t(1));
    peers.apply_block_delivery(alice, hash(1), BlockHeight::from(1), None, t(2), Duration::from_millis(5), 100);

    let claimants = peers.apply_direct_claimants(&hash(1));
    assert_eq!(claimants.len(), 1);
    assert_eq!(claimants[0].2, ClaimKind::BlockDelivery);
}

#[test]
fn header_lag_records_zero_for_first_announcer_and_delay_for_late() {
    let mut peers = PeerPerformance::new();
    let alice = peer("alice");
    let bob = peer("bob");

    peers.apply_header_announcement(alice, tip(1, 1), None, t(1));
    peers.apply_header_announcement(bob, tip(1, 1), None, t(3));

    assert_eq!(peers.apply_scores(&alice).header_lag_ewma, Some(Duration::ZERO));
    assert_eq!(peers.apply_scores(&bob).header_lag_ewma, Some(Duration::from_secs(2)));
}

// ---------------------------------------------------------------------------
// Header lifecycle (local HeaderPerformance)
// ---------------------------------------------------------------------------

#[test]
fn header_received_and_peer_claim_are_independent_maps() {
    let mut peers = PeerPerformance::new();
    let mut headers = HeaderPerformance::new();
    let alice = peer("alice");

    peers.apply_header_announcement(alice, tip(1, 1), None, t(1));
    headers.apply_header_received(alice, tip(1, 1), t(1), 1_000, false);

    assert_eq!(headers.lifecycle_count(), 1);
    assert!(peers.apply_peer_covers_fragment(&alice, &[hash(1)]));
    assert_eq!(peers.apply_first_announced_at(&hash(1)).map(|(p, _)| p), Some(alice));
}

#[test]
fn first_header_announcer_peer_is_retained() {
    let mut headers = HeaderPerformance::new();
    let alice = peer("alice");
    let bob = peer("bob");

    headers.apply_header_received(alice, tip(1, 1), t(1), 1_000, false);
    // Later announcer must not overwrite the first peer or slot interval.
    headers.apply_header_received(bob, tip(1, 1), t(2), 9_999, false);

    assert_eq!(headers.first_announcer(&hash(1)), Some(alice));
    assert_eq!(headers.slot_start_to_header_micros(&hash(1)), Some(1_000));
}

#[test]
fn blocks_requested_and_downloaded_then_valid_closes_lifecycle() {
    let mut headers = HeaderPerformance::new();
    let alice = peer("alice");
    headers.apply_header_received(alice, tip(1, 1), t(1), 500, false);
    headers.apply_blocks_requested(&[hash(1)], t(2));
    let received = headers.apply_block_downloaded(alice, &hash(1), BlockHeight::from(1), t(3));
    assert!(matches!(
        &received[..],
        [super::HeaderTelemetry::Received { rank: 1, peer, .. }] if *peer == alice
    ));
    assert_eq!(headers.lifecycle_count(), 1);
    let telemetry = headers.apply_block_valid(&hash(1), t(4), false);
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
    headers.apply_header_received(peer("alice"), tip(1, 1), t(1), 0, false);
    assert!(headers.apply_fork_started(tip(1, 1), t(1)).is_empty());
    assert!(headers.has_fork_switch(&hash(1)));
    let telemetry = headers.apply_block_valid(&hash(1), t(2), false);
    assert!(!headers.has_fork_switch(&hash(1)));
    assert_eq!(telemetry.len(), 3); // lifecycle + adopted + fork switch
}

#[test]
fn slot_start_metric_omitted_while_syncing() {
    let mut headers = HeaderPerformance::new();
    headers.apply_header_received(peer("alice"), tip(1, 1), t(1), 42_000, false);
    let telemetry = headers.apply_block_valid(&hash(1), t(2), true);
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
    let announced: Vec<_> =
        peers.iter().flat_map(|peer| headers.apply_header_received(*peer, tip(1, 1), t(1), 0, false)).collect();
    assert_eq!(announced.len(), 3);
    assert!(matches!(&announced[0], super::HeaderTelemetry::Announced { rank: 1, peer, .. } if *peer == peers[0]));
    assert!(matches!(&announced[2], super::HeaderTelemetry::Announced { rank: 3, peer, .. } if *peer == peers[2]));
    assert!(headers.apply_header_received(peers[0], tip(1, 1), t(2), 0, false).is_empty());

    let first = headers.apply_block_downloaded(peers[2], &hash(1), BlockHeight::from(1), t(3));
    assert!(headers.apply_block_downloaded(peers[2], &hash(1), BlockHeight::from(1), t(4)).is_empty());
    let second = headers.apply_block_downloaded(peers[0], &hash(1), BlockHeight::from(1), t(5));
    assert!(matches!(&first[..], [super::HeaderTelemetry::Received { rank: 1, peer, .. }] if *peer == peers[2]));
    assert!(matches!(&second[..], [super::HeaderTelemetry::Received { rank: 2, peer, .. }] if *peer == peers[0]));

    let adopted = headers.apply_block_valid(&hash(1), t(6), false);
    assert!(
        matches!(adopted.last(), Some(super::HeaderTelemetry::Adopted { peer: Some(peer), .. }) if *peer == peers[2])
    );
}

#[test]
fn already_stored_header_does_not_open_a_rank_one_announcement() {
    let mut headers = HeaderPerformance::new();
    let alice = peer("alice");
    let bob = peer("bob");

    let first = headers.apply_header_received(alice, tip(1, 1), t(1), 1_000, false);
    assert!(matches!(&first[..], [super::HeaderTelemetry::Announced { rank: 1, .. }]));

    // A later peer extends the open list and leaves the first announcer's interval in place.
    let second = headers.apply_header_received(bob, tip(1, 1), t(2), 9_000, true);
    assert!(matches!(&second[..], [super::HeaderTelemetry::Announced { rank: 2, peer, .. }] if *peer == bob));
    assert_eq!(headers.first_announcer(&hash(1)), Some(alice));
    assert_eq!(headers.slot_start_to_header_micros(&hash(1)), Some(1_000));

    headers.apply_block_valid(&hash(1), t(3), false);
    assert!(
        headers.apply_header_received(peer("carol"), tip(1, 1), t(4), 1, true).is_empty(),
        "an adopted header is not announced again"
    );
    assert!(headers.apply_header_received(peer("dave"), tip(1, 1), t(4), 1, false).is_empty());
    headers.apply_prune_below(BlockHeight::from(2), t(5));
    assert!(headers.apply_header_received(peer("carol"), tip(1, 1), t(5), 1, true).is_empty());
    assert_eq!(headers.lifecycle_count(), 0);

    // A stored header this node never collected announcers for stays silent.
    assert!(headers.apply_header_received(alice, tip(9, 9), t(6), 1, true).is_empty());
    assert_eq!(headers.lifecycle_count(), 0);
}

#[test]
fn prune_below_closes_open_lifecycles_as_pruned() {
    let mut headers = HeaderPerformance::new();
    headers.apply_header_received(peer("alice"), tip(1, 1), t(1), 0, false);
    headers.apply_header_received(peer("alice"), tip(5, 5), t(2), 0, false);
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
            let effect = Performance::record_header_announcement(*peer, point, None, t(1), 0, false);
            rt.block_on(Box::new(effect).run(resources.clone()));
        }
        super::emit_blocks_requested(&[body], &peers[..2], live);
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
        let effect = Performance::record_block_valid(body, t(3), false);
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
