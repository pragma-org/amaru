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

use std::collections::BTreeSet;

use amaru_kernel::{BlockHeader, BlockHeight, IsHeader, Peer, Point, Tip, make_header};
use tracing::Level;

use super::{
    BlockSourceMsg, BlockValidity,
    test_setup::{assert_trace, setup, te_send, test_prep},
};
use crate::stages::{
    peer_selection::PeerSelectionMsg,
    test_utils::{te_input, te_state},
};

const BS: &str = "bs-1";

fn point_n(n: u64) -> Point {
    BlockHeader::from(make_header(n, n, None)).point()
}

fn tip_adopted(h: u64) -> Tip {
    Tip::new(point_n(h), BlockHeight::from(h))
}

#[test]
fn test_block_received_inserts_pending() {
    let prep = test_prep(tip_adopted(100), 1000);
    let p = point_n(50);
    let m = BlockSourceMsg::BlockReceived { peer: Peer::new("alice"), point: p, block_height: BlockHeight::from(50) };
    let mut expected = prep.state.clone();
    expected.by_point.insert(p, BlockValidity::Pending(BlockHeight::from(50), BTreeSet::from([Peer::new("alice")])));
    let (running, _g, mut logs) = setup(&prep, std::slice::from_ref(&m));
    assert_trace(&running, &[te_state(BS, &prep.state), te_input(BS, &m), te_state(BS, &expected)]);
    logs.assert_no_remaining_at([Level::WARN, Level::ERROR]);
}

#[test]
fn test_block_received_merges_second_peer() {
    let prep = test_prep(tip_adopted(100), 1000);
    let p = point_n(50);
    let m1 = BlockSourceMsg::BlockReceived { peer: Peer::new("alice"), point: p, block_height: BlockHeight::from(50) };
    let m2 = BlockSourceMsg::BlockReceived { peer: Peer::new("bob"), point: p, block_height: BlockHeight::from(50) };
    let mut expected = prep.state.clone();
    expected.by_point.insert(
        p,
        BlockValidity::Pending(BlockHeight::from(50), BTreeSet::from([Peer::new("alice"), Peer::new("bob")])),
    );
    let (running, _g, mut logs) = setup(&prep, &[m1.clone(), m2.clone()]);
    assert_trace(
        &running,
        &[
            te_state(BS, &prep.state),
            te_input(BS, &m1),
            te_state(BS, &{
                let mut s = prep.state.clone();
                s.by_point
                    .insert(p, BlockValidity::Pending(BlockHeight::from(50), BTreeSet::from([Peer::new("alice")])));
                s
            }),
            te_input(BS, &m2),
            te_state(BS, &expected),
        ],
    );
    logs.assert_no_remaining_at([Level::WARN, Level::ERROR]);
}

#[test]
fn test_validation_valid_marks_known_point() {
    let prep = test_prep(tip_adopted(100), 1000);
    let p = point_n(50);
    let m1 = BlockSourceMsg::BlockReceived { peer: Peer::new("alice"), point: p, block_height: BlockHeight::from(50) };
    let m2 = BlockSourceMsg::Validation { valid: true, point: p };
    let mut expected = prep.state.clone();
    expected.by_point.insert(p, BlockValidity::Valid(BlockHeight::from(50)));
    let (running, _g, mut logs) = setup(&prep, &[m1.clone(), m2.clone()]);
    assert_trace(
        &running,
        &[
            te_state(BS, &prep.state),
            te_input(BS, &m1),
            te_state(BS, &{
                let mut s = prep.state.clone();
                s.by_point
                    .insert(p, BlockValidity::Pending(BlockHeight::from(50), BTreeSet::from([Peer::new("alice")])));
                s
            }),
            te_input(BS, &m2),
            te_state(BS, &expected),
        ],
    );
    logs.assert_no_remaining_at([Level::WARN, Level::ERROR]);
}

#[test]
fn test_validation_valid_unknown_point_is_noop() {
    let prep = test_prep(tip_adopted(100), 1000);
    let p = point_n(99);
    let m = BlockSourceMsg::Validation { valid: true, point: p };
    let (running, _g, mut logs) = setup(&prep, std::slice::from_ref(&m));
    assert_trace(&running, &[te_state(BS, &prep.state), te_input(BS, &m), te_state(BS, &prep.state)]);
    logs.assert_no_remaining_at([Level::WARN, Level::ERROR]);
}

#[test]
fn test_validation_invalid_faults_each_peer() {
    let prep = test_prep(tip_adopted(100), 1000);
    let p = point_n(50);
    let m1 = BlockSourceMsg::BlockReceived { peer: Peer::new("alice"), point: p, block_height: BlockHeight::from(50) };
    let m2 = BlockSourceMsg::BlockReceived { peer: Peer::new("bob"), point: p, block_height: BlockHeight::from(50) };
    let m3 = BlockSourceMsg::Validation { valid: false, point: p };
    let mut after_pending_bob = prep.state.clone();
    after_pending_bob
        .by_point
        .insert(p, BlockValidity::Pending(BlockHeight::from(50), BTreeSet::from([Peer::new("alice")])));
    let mut after_both_pending = prep.state.clone();
    after_both_pending.by_point.insert(
        p,
        BlockValidity::Pending(BlockHeight::from(50), BTreeSet::from([Peer::new("alice"), Peer::new("bob")])),
    );
    let mut final_state = prep.state.clone();
    final_state.by_point.insert(p, BlockValidity::Invalid(BlockHeight::from(50)));
    let (running, _g, mut logs) = setup(&prep, &[m1.clone(), m2.clone(), m3.clone()]);
    assert_trace(
        &running,
        &[
            te_state(BS, &prep.state),
            te_input(BS, &m1),
            te_state(BS, &after_pending_bob),
            te_input(BS, &m2),
            te_state(BS, &after_both_pending),
            te_input(BS, &m3),
            te_send(BS, "invalid_sink", PeerSelectionMsg::Adversarial(Peer::new("alice"))),
            te_send(BS, "invalid_sink", PeerSelectionMsg::Adversarial(Peer::new("bob"))),
            te_state(BS, &final_state),
        ],
    );
    logs.assert_no_remaining_at([Level::WARN, Level::ERROR]);
}

#[test]
fn test_validation_invalid_without_provenance_sends_nothing() {
    let prep = test_prep(tip_adopted(100), 1000);
    let p = point_n(77);
    let m = BlockSourceMsg::Validation { valid: false, point: p };
    let (running, _g, mut logs) = setup(&prep, std::slice::from_ref(&m));
    assert_trace(&running, &[te_state(BS, &prep.state), te_input(BS, &m), te_state(BS, &prep.state)]);
    logs.assert_no_remaining_at([Level::WARN, Level::ERROR]);
}

#[test]
fn test_block_received_after_invalid_new_peer_faults() {
    let prep = test_prep(tip_adopted(100), 1000);
    let p = point_n(50);
    let m1 = BlockSourceMsg::BlockReceived { peer: Peer::new("alice"), point: p, block_height: BlockHeight::from(50) };
    let m2 = BlockSourceMsg::Validation { valid: false, point: p };
    let m3 = BlockSourceMsg::BlockReceived { peer: Peer::new("carol"), point: p, block_height: BlockHeight::from(50) };
    let mut after_alice = prep.state.clone();
    after_alice.by_point.insert(p, BlockValidity::Pending(BlockHeight::from(50), BTreeSet::from([Peer::new("alice")])));
    let mut after_invalid = prep.state.clone();
    after_invalid.by_point.insert(p, BlockValidity::Invalid(BlockHeight::from(50)));
    let mut after_carol = prep.state.clone();
    after_carol.by_point.insert(p, BlockValidity::Invalid(BlockHeight::from(50)));
    let (running, _g, mut logs) = setup(&prep, &[m1.clone(), m2.clone(), m3.clone()]);
    assert_trace(
        &running,
        &[
            te_state(BS, &prep.state),
            te_input(BS, &m1),
            te_state(BS, &after_alice),
            te_input(BS, &m2),
            te_send(BS, "invalid_sink", PeerSelectionMsg::Adversarial(Peer::new("alice"))),
            te_state(BS, &after_invalid),
            te_input(BS, &m3),
            te_send(BS, "invalid_sink", PeerSelectionMsg::Adversarial(Peer::new("carol"))),
            te_state(BS, &after_carol),
        ],
    );
    logs.assert_no_remaining_at([Level::WARN, Level::ERROR]);
}

#[test]
fn test_block_received_after_invalid_same_peer_no_second_fault() {
    let prep = test_prep(tip_adopted(100), 1000);
    let p = point_n(50);
    let m1 = BlockSourceMsg::BlockReceived { peer: Peer::new("alice"), point: p, block_height: BlockHeight::from(50) };
    let m2 = BlockSourceMsg::Validation { valid: false, point: p };
    let m3 = BlockSourceMsg::BlockReceived { peer: Peer::new("alice"), point: p, block_height: BlockHeight::from(50) };
    let mut after_alice = prep.state.clone();
    after_alice.by_point.insert(p, BlockValidity::Pending(BlockHeight::from(50), BTreeSet::from([Peer::new("alice")])));
    let mut after_invalid = prep.state.clone();
    after_invalid.by_point.insert(p, BlockValidity::Invalid(BlockHeight::from(50)));
    let (running, _g, mut logs) = setup(&prep, &[m1.clone(), m2.clone(), m3.clone()]);
    assert_trace(
        &running,
        &[
            te_state(BS, &prep.state),
            te_input(BS, &m1),
            te_state(BS, &after_alice),
            te_input(BS, &m2),
            te_send(BS, "invalid_sink", PeerSelectionMsg::Adversarial(Peer::new("alice"))),
            te_state(BS, &after_invalid),
            te_input(BS, &m3),
            te_state(BS, &after_invalid),
        ],
    );
    logs.assert_no_remaining_at([Level::WARN, Level::ERROR]);
}

#[test]
fn test_block_received_pruned_when_below_adopted_window() {
    let prep = test_prep(tip_adopted(100), 10);
    let p = point_n(50);
    let m = BlockSourceMsg::BlockReceived { peer: Peer::new("alice"), point: p, block_height: BlockHeight::from(50) };
    let (running, _g, mut logs) = setup(&prep, std::slice::from_ref(&m));
    assert_trace(&running, &[te_state(BS, &prep.state), te_input(BS, &m), te_state(BS, &prep.state)]);
    logs.assert_no_remaining_at([Level::WARN, Level::ERROR]);
}

#[test]
fn test_adopted_tip_prunes_entries_far_below() {
    let prep = test_prep(tip_adopted(100), 100);
    let p = point_n(50);
    let m1 = BlockSourceMsg::BlockReceived { peer: Peer::new("alice"), point: p, block_height: BlockHeight::from(50) };
    let m2 = BlockSourceMsg::AdoptedTip(tip_adopted(200));
    let mut after_recv = prep.state.clone();
    after_recv.by_point.insert(p, BlockValidity::Pending(BlockHeight::from(50), BTreeSet::from([Peer::new("alice")])));
    let mut after_adopt = prep.state.clone();
    after_adopt.adopted_tip = tip_adopted(200);
    after_adopt.max_tip_distance = 100;
    let (running, _g, mut logs) = setup(&prep, &[m1.clone(), m2.clone()]);
    assert_trace(
        &running,
        &[
            te_state(BS, &prep.state),
            te_input(BS, &m1),
            te_state(BS, &after_recv),
            te_input(BS, &m2),
            te_state(BS, &after_adopt),
        ],
    );
    logs.assert_no_remaining_at([Level::WARN, Level::ERROR]);
}

#[test]
fn test_adopted_tip_updates_only() {
    let prep = test_prep(tip_adopted(100), 1000);
    let m = BlockSourceMsg::AdoptedTip(tip_adopted(150));
    let mut expected = prep.state.clone();
    expected.adopted_tip = tip_adopted(150);
    let (running, _g, mut logs) = setup(&prep, std::slice::from_ref(&m));
    assert_trace(&running, &[te_state(BS, &prep.state), te_input(BS, &m), te_state(BS, &expected)]);
    logs.assert_no_remaining_at([Level::WARN, Level::ERROR]);
}
