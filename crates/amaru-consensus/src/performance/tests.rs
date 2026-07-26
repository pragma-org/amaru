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

use std::{sync::Arc, time::Duration};

use amaru_kernel::{BlockHeight, HeaderHash, Peer, Point, Slot, Tip};
use amaru_pure_stage::{ExternalEffect, Instant, Resources};

use super::{ClaimKind, HeaderLifecycleOutcome, Performance, ResourcePerformance, SelectPeersParams};

fn t(secs: u64) -> Instant {
    Instant::at_offset(Duration::from_secs(secs), Duration::ZERO)
}

fn hash(byte: u8) -> HeaderHash {
    HeaderHash::from([byte; 32])
}

fn tip(byte: u8, height: u64) -> Tip {
    Tip::new(Point::Specific(Slot::from(height), hash(byte)), BlockHeight::from(height))
}

fn peer(name: &str) -> Peer {
    Peer::new(name)
}

fn select(need: Vec<HeaderHash>, max_peers: usize) -> SelectPeersParams {
    SelectPeersParams { need, max_peers, min_peers: 1, now: t(100) }
}

// ---------------------------------------------------------------------------
// Coverage semantics
// ---------------------------------------------------------------------------

#[test]
fn intersect_only_covers_need_ending_at_intersect_not_unknown_child() {
    let perf = Performance::new();
    let alice = peer("alice");
    let h1 = tip(1, 1);

    perf.apply_intersection(alice.clone(), h1, None, t(1));

    assert!(perf.apply_peer_covers_fragment(&alice, &[hash(1)]));
    assert!(!perf.apply_peer_covers_fragment(&alice, &[hash(1), hash(2)]));
    assert!(!perf.apply_peer_covers_fragment(&alice, &[hash(2)]));
}

#[test]
fn announce_chain_collapses_to_single_tip_and_covers_ancestors() {
    let perf = Performance::new();
    let alice = peer("alice");

    perf.apply_header_announcement(alice.clone(), tip(1, 1), None, t(1));
    perf.apply_header_announcement(alice.clone(), tip(2, 2), Some(hash(1)), t(2));

    let snap = perf.apply_snapshot(&alice).expect("alice present");
    assert_eq!(snap.tips.len(), 1);
    assert_eq!(snap.tips[0].hash, hash(2));

    assert!(perf.apply_peer_covers_fragment(&alice, &[hash(1)]));
    assert!(perf.apply_peer_covers_fragment(&alice, &[hash(1), hash(2)]));
    assert!(perf.apply_peer_covers_fragment(&alice, &[hash(2)]));
}

#[test]
fn duplicate_announcers_both_selected() {
    let perf = Performance::new();
    let alice = peer("alice");
    let bob = peer("bob");

    perf.apply_header_announcement(alice.clone(), tip(1, 1), None, t(1));
    perf.apply_header_announcement(bob.clone(), tip(1, 1), None, t(2));

    let set = perf.apply_select_peers_for_fetch(select(vec![hash(1)], 2));
    assert!(!set.weak);
    assert_eq!(set.peers.len(), 2);
    assert!(set.peers.contains(&alice));
    assert!(set.peers.contains(&bob));

    let first = perf.apply_first_announced_at(&hash(1)).expect("first announcer");
    assert_eq!(first.0, alice);
    assert_eq!(first.1, t(1));
}

#[test]
fn descendant_claim_covers_ancestor_via_parent_walk() {
    let perf = Performance::new();
    let alice = peer("alice");

    perf.apply_header_announcement(alice.clone(), tip(1, 1), None, t(1));
    perf.apply_header_announcement(alice.clone(), tip(2, 2), Some(hash(1)), t(2));
    perf.apply_header_announcement(alice.clone(), tip(3, 3), Some(hash(2)), t(3));

    assert!(perf.apply_peer_covers_fragment(&alice, &[hash(1)]));
    assert!(perf.apply_peer_covers_fragment(&alice, &[hash(2)]));
    assert!(perf.apply_peer_covers_fragment(&alice, &[hash(1), hash(2), hash(3)]));
}

#[test]
fn parent_walk_stops_at_target_height_on_wrong_branch() {
    let perf = Performance::new();
    let alice = peer("alice");
    let bob = peer("bob");

    // Alice has a chain at heights 1..=5 (hashes 1..=5).
    for h in 1u8..=5 {
        let parent = (h > 1).then(|| hash(h - 1));
        perf.apply_header_announcement(alice.clone(), tip(h, h as u64), parent, t(h as u64));
    }
    // Bob claims a different block at height 3 (hash 30). Alice's branch has hash 3 there.
    perf.apply_intersection(bob, tip(30, 3), Some(hash(2)), t(10));

    // Alice must not cover Bob's height-3 block; height early-exit avoids walking past height 3.
    assert!(!perf.apply_peer_covers_fragment(&alice, &[hash(30)]));
    // Alice still covers her own tip / ancestors.
    assert!(perf.apply_peer_covers_fragment(&alice, &[hash(5)]));
    assert!(perf.apply_peer_covers_fragment(&alice, &[hash(2)]));
}

#[test]
fn ancestor_only_claim_does_not_cover_descendant() {
    let perf = Performance::new();
    let alice = peer("alice");

    perf.apply_intersection(alice.clone(), tip(1, 1), None, t(1));

    assert!(perf.apply_peer_covers_fragment(&alice, &[hash(1)]));
    assert!(!perf.apply_peer_covers_fragment(&alice, &[hash(2)]));
    assert!(!perf.apply_peer_covers_fragment(&alice, &[hash(1), hash(2)]));
}

#[test]
fn intersect_at_tip_of_need_covers_full_fragment_via_index() {
    let perf = Performance::new();
    let alice = peer("alice");

    // Recovery-style: need is [H1, H2, H3], peer intersects at H3 without intermediate announcements.
    perf.apply_intersection(alice.clone(), tip(3, 3), Some(hash(2)), t(1));

    assert!(perf.apply_peer_covers_fragment(&alice, &[hash(1), hash(2), hash(3)]));
    assert!(perf.apply_peer_covers_fragment(&alice, &[hash(3)]));
}

// ---------------------------------------------------------------------------
// Rollback, prune, lifecycle
// ---------------------------------------------------------------------------

#[test]
fn rollback_drops_fork_tip_and_restores_point() {
    let perf = Performance::new();
    let alice = peer("alice");

    perf.apply_header_announcement(alice.clone(), tip(1, 1), None, t(1));
    perf.apply_header_announcement(alice.clone(), tip(2, 2), Some(hash(1)), t(2));
    // Fork side-branch tip
    perf.apply_header_announcement(alice.clone(), tip(10, 3), Some(hash(1)), t(3));

    let before = perf.apply_snapshot(&alice).expect("alice");
    assert!(!before.tips.is_empty());

    perf.apply_rollback(alice.clone(), tip(1, 1), None, t(4));

    let after = perf.apply_snapshot(&alice).expect("alice");
    assert!(after.tips.iter().any(|c| c.hash == hash(1)));
    assert!(!after.tips.iter().any(|c| c.hash == hash(2)));
    assert!(!after.tips.iter().any(|c| c.hash == hash(10)));
    assert!(perf.apply_peer_covers_fragment(&alice, &[hash(1)]));
    assert!(!perf.apply_peer_covers_fragment(&alice, &[hash(2)]));
}

#[test]
fn prune_removes_old_tips_but_retains_scores() {
    let perf = Performance::new();
    let alice = peer("alice");

    perf.apply_header_announcement(alice.clone(), tip(1, 1), None, t(1));
    perf.apply_block_delivery(
        alice.clone(),
        hash(1),
        BlockHeight::from(1),
        None,
        t(2),
        Duration::from_millis(50),
        90_000,
    );

    let scores_before = perf.apply_scores(&alice);
    assert!(scores_before.block_response_ewma.is_some());
    assert_eq!(scores_before.fetch_successes, 1);

    perf.apply_prune_below(BlockHeight::from(5));

    let snap = perf.apply_snapshot(&alice).expect("alice kept for scores");
    assert!(snap.tips.is_empty());
    assert!(!perf.apply_peer_covers_fragment(&alice, &[hash(1)]));
    assert_eq!(perf.apply_scores(&alice).fetch_successes, 1);
    assert!(perf.apply_scores(&alice).block_response_ewma.is_some());
}

#[test]
fn clear_availability_keeps_scores_forget_removes_all() {
    let perf = Performance::new();
    let alice = peer("alice");

    perf.apply_header_announcement(alice.clone(), tip(1, 1), None, t(1));
    perf.apply_block_delivery(
        alice.clone(),
        hash(1),
        BlockHeight::from(1),
        None,
        t(2),
        Duration::from_millis(20),
        1000,
    );

    perf.apply_clear_peer_availability(&alice);
    assert!(!perf.apply_peer_covers_fragment(&alice, &[hash(1)]));
    assert_eq!(perf.apply_scores(&alice).fetch_successes, 1);
    assert!(perf.apply_direct_claimants(&hash(1)).is_empty());

    perf.apply_forget_peer(&alice);
    assert!(perf.apply_snapshot(&alice).is_none());
    assert_eq!(perf.apply_scores(&alice).fetch_successes, 0);
}

// ---------------------------------------------------------------------------
// Ranking and selection
// ---------------------------------------------------------------------------

#[test]
fn ranking_prefers_faster_delivery() {
    let perf = Performance::new();
    let fast = peer("fast");
    let slow = peer("slow");
    let partial = peer("partial");

    // range-capable, fast
    perf.apply_header_announcement(fast.clone(), tip(1, 1), None, t(1));
    perf.apply_header_announcement(fast.clone(), tip(2, 2), Some(hash(1)), t(2));
    perf.apply_block_delivery(
        fast.clone(),
        hash(2),
        BlockHeight::from(2),
        Some(hash(1)),
        t(3),
        Duration::from_millis(10),
        90_000,
    );

    // range-capable, slow
    perf.apply_header_announcement(slow.clone(), tip(1, 1), None, t(1));
    perf.apply_header_announcement(slow.clone(), tip(2, 2), Some(hash(1)), t(2));
    perf.apply_block_delivery(
        slow.clone(),
        hash(2),
        BlockHeight::from(2),
        Some(hash(1)),
        t(4),
        Duration::from_secs(2),
        90_000,
    );

    // only covers head of the range — must not be selected for a multi-block fetch
    perf.apply_header_announcement(partial.clone(), tip(1, 1), None, t(1));

    let set = perf.apply_select_peers_for_fetch(select(vec![hash(1), hash(2)], 3));
    assert!(!set.weak);
    assert_eq!(set.peers, vec![fast, slow]);
    assert!(!set.peers.contains(&partial));
}

#[test]
fn prefix_only_peer_not_selected_for_range() {
    let perf = Performance::new();
    let prefix = peer("prefix");
    let full = peer("full");

    perf.apply_header_announcement(prefix.clone(), tip(1, 1), None, t(1));
    perf.apply_header_announcement(full.clone(), tip(1, 1), None, t(1));
    perf.apply_header_announcement(full.clone(), tip(2, 2), Some(hash(1)), t(2));

    let set = perf.apply_select_peers_for_fetch(select(vec![hash(1), hash(2)], 5));
    assert_eq!(set.peers, vec![full]);
    assert!(!set.peers.contains(&prefix));
}

#[test]
fn cold_start_empty_map_returns_weak_empty_selection() {
    let perf = Performance::new();
    let set = perf.apply_select_peers_for_fetch(select(vec![hash(1)], 5));
    assert!(set.weak);
    assert!(set.peers.is_empty());
}

#[test]
fn after_intersect_selection_becomes_non_empty() {
    let perf = Performance::new();
    let alice = peer("alice");

    let empty = perf.apply_select_peers_for_fetch(select(vec![hash(5)], 5));
    assert!(empty.weak);

    perf.apply_intersection(alice.clone(), tip(5, 5), Some(hash(4)), t(1));

    let set = perf.apply_select_peers_for_fetch(select(vec![hash(5)], 5));
    assert!(!set.weak);
    assert_eq!(set.peers, vec![alice]);
}

#[test]
fn max_peers_bounds_selection() {
    let perf = Performance::new();
    for i in 0..10u8 {
        let p = peer(&format!("p{i}"));
        perf.apply_header_announcement(p, tip(1, 1), None, t(1));
    }
    let set = perf.apply_select_peers_for_fetch(select(vec![hash(1)], 3));
    assert_eq!(set.peers.len(), 3);
    assert!(!set.weak);
}

#[test]
fn churn_ranks_unreliable_peers_first() {
    let perf = Performance::new();
    let good = peer("good");
    let bad = peer("bad");

    perf.apply_header_announcement(good.clone(), tip(1, 1), None, t(1));
    perf.apply_block_delivery(good.clone(), hash(1), BlockHeight::from(1), None, t(2), Duration::from_millis(10), 1000);

    perf.apply_header_announcement(bad.clone(), tip(1, 1), None, t(1));
    perf.apply_fetch_failure(std::slice::from_ref(&bad), t(3));
    perf.apply_fetch_failure(std::slice::from_ref(&bad), t(4));

    let ranked = perf.apply_rank_peers_for_churn(&[good.clone(), bad.clone()], t(5));
    assert_eq!(ranked[0].0, bad);
    assert_eq!(ranked[1].0, good);
}

#[test]
fn claim_kind_strength_prefers_delivery_over_intersection() {
    let perf = Performance::new();
    let alice = peer("alice");

    perf.apply_intersection(alice.clone(), tip(1, 1), None, t(1));
    perf.apply_block_delivery(alice.clone(), hash(1), BlockHeight::from(1), None, t(2), Duration::from_millis(5), 100);

    let claimants = perf.apply_direct_claimants(&hash(1));
    assert_eq!(claimants.len(), 1);
    assert_eq!(claimants[0].2, ClaimKind::BlockDelivery);
}

#[test]
fn header_lag_records_zero_for_first_announcer_and_delay_for_late() {
    let perf = Performance::new();
    let alice = peer("alice");
    let bob = peer("bob");

    perf.apply_header_announcement(alice.clone(), tip(1, 1), None, t(1));
    perf.apply_header_announcement(bob.clone(), tip(1, 1), None, t(3));

    assert_eq!(perf.apply_scores(&alice).header_lag_ewma, Some(Duration::ZERO));
    assert_eq!(perf.apply_scores(&bob).header_lag_ewma, Some(Duration::from_secs(2)));
}

// ---------------------------------------------------------------------------
// External effects (stage-facing static constructors)
// ---------------------------------------------------------------------------

#[test]
fn effect_constructors_mutate_resource_via_run() {
    let perf = Arc::new(Performance::new());
    let resources = Resources::default();
    resources.put::<ResourcePerformance>(perf.clone());

    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();

    let alice = peer("alice");
    let effect = Performance::record_intersection(alice.clone(), tip(7, 7), None, t(1));
    let _ = rt.block_on(Box::new(effect).run(resources.clone()));

    assert!(perf.apply_peer_covers_fragment(&alice, &[hash(7)]));

    let select_effect = Performance::select_peers_for_fetch(select(vec![hash(7)], 5));
    let response = rt.block_on(Box::new(select_effect).run(resources));
    let set = *response.cast::<super::FetchPeerSet>().expect("FetchPeerSet response");
    assert!(!set.weak);
    assert_eq!(set.peers, vec![alice]);
}

// ---------------------------------------------------------------------------
// Header lifecycle (unified events)
// ---------------------------------------------------------------------------

#[test]
fn header_announcement_starts_lifecycle_and_records_peer_claim() {
    let perf = Performance::new();
    let alice = peer("alice");
    perf.apply_header_announcement(alice.clone(), tip(1, 1), None, t(1));
    assert_eq!(perf.headers().lifecycle_count(), 1);
    assert!(perf.apply_peer_covers_fragment(&alice, &[hash(1)]));
    assert_eq!(perf.apply_first_announced_at(&hash(1)).map(|(p, _)| p), Some(alice));
}

#[test]
fn blocks_requested_and_downloaded_then_valid_closes_lifecycle() {
    let perf = Performance::new();
    let alice = peer("alice");
    perf.apply_header_announcement(alice.clone(), tip(1, 1), None, t(1));
    perf.apply_blocks_requested(&[hash(1)], t(2));
    perf.apply_block_delivery(alice, hash(1), BlockHeight::from(1), None, t(3), Duration::from_millis(50), 1000);
    assert_eq!(perf.headers().lifecycle_count(), 1);
    perf.apply_block_valid(&hash(1), t(4), None);
    assert_eq!(perf.headers().lifecycle_count(), 0);
}

#[test]
fn header_rejected_does_not_require_lifecycle_entry() {
    let perf = Performance::new();
    perf.apply_header_rejected(HeaderLifecycleOutcome::DuplicateHeader, None);
    assert_eq!(perf.headers().lifecycle_count(), 0);
}

#[test]
fn fork_started_and_closed_on_valid_block() {
    let perf = Performance::new();
    let alice = peer("alice");
    perf.apply_header_announcement(alice, tip(1, 1), None, t(1));
    perf.apply_fork_started(tip(1, 1), t(1), None);
    assert!(perf.headers().has_fork_switch(&hash(1)));
    perf.apply_block_valid(&hash(1), t(2), None);
    assert!(!perf.headers().has_fork_switch(&hash(1)));
}
