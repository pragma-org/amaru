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

use std::{
    slice,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use amaru_kernel::{BlockHeight, Epoch, EraName, HeaderHash, IsHeader, Peer, Point, Tip};
use amaru_ouroboros::praos::header::AssertHeaderError;
use amaru_ouroboros_traits::has_stake_distribution::GetPoolError;
use amaru_protocols::chainsync::{
    self, ChainSyncInitiatorMsg, HeaderContent, InitiatorMessage, InitiatorMessage::RequestNext,
};
use amaru_pure_stage::{
    assert_trace_contains, assert_trace_does_not_contain, assert_trace_match, simulation::running::OverrideResult,
    tm_send, trace_match::tm_clock,
};
use tracing::Level;

use crate::{
    effects::{TipEffect, ValidateHeaderEffect, VolatileTipEffect},
    stages::{
        peer_selection::PeerSelectionMsg,
        test_utils::{assert_trace, te_input, te_send, te_state, tm_state},
        track_peers::{
            TrackPeers, TrackPeersMsg,
            test_setup::{
                build_store, make_block_header, setup, setup_base, setup_with_ledger_tip, te_clock_suspend,
                te_has_header, te_load_tip, te_store_header, te_validate_header, test_prep,
                test_prep_with_security_param, tm_store_header, tm_volatile_tip,
            },
        },
    },
    store::NoncesError,
    validate_header::ValidateHeaderError,
};

#[test]
fn test_new_peer() {
    let prep = test_prep();
    let state = prep.state.clone();
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: Peer::new("peer1"),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::Initialize,
    });

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace(&running, &[te_state("tp-1", &state), te_input("tp-1", &msg), te_state("tp-1", &state)]);
    logs.assert_and_remove(Level::INFO, &["initializing chainsync"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

#[test]
fn test_initialize_existing_peer() {
    let prep = test_prep();
    let peer = Peer::new("peer1");
    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), Tip::origin(), Tip::origin());
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::Initialize,
    });

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace(&running, &[te_state("tp-1", &state), te_input("tp-1", &msg), te_state("tp-1", &state)]);
    logs.assert_and_remove(Level::INFO, &["initializing chainsync"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

#[test]
fn test_intersect_found_missing_header_sends_done() {
    let prep = test_prep();
    let state = prep.state.clone();
    let current = Point::Specific(1u64.into(), HeaderHash::from([1u8; 32]));
    let tip = Tip::new(current, BlockHeight::from(1));
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: Peer::new("peer1"),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::IntersectFound(current, tip),
    });

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace(
        &running,
        &[
            te_state("tp-1", &state),
            te_input("tp-1", &msg),
            te_load_tip("tp-1", current.hash()),
            te_send("tp-1", &prep.handler, chainsync::InitiatorMessage::Done),
            te_state("tp-1", &state),
        ],
    );
    logs.assert_and_remove(Level::WARN, &["peer sent unknown intersection point"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

#[test]
fn test_intersect_found_tracks_peer() {
    let prep = test_prep();
    let state = prep.state.clone();
    let header = &prep.headers[0];
    let current = header.point();
    let tip = prep.headers[1].tip();
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: Peer::new("peer1"),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::IntersectFound(current, tip),
    });

    let mut expected = state.clone();
    expected.insert_peer(Peer::new("peer1"), header.tip(), tip);

    let (running, _guards, mut logs) =
        setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(slice::from_ref(header)));
    assert_trace(
        &running,
        &[
            te_state("tp-1", &state),
            te_input("tp-1", &msg),
            te_load_tip("tp-1", current.hash()),
            te_state("tp-1", &expected),
        ],
    );
    logs.assert_and_remove(Level::INFO, &["intersect found"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

#[test]
fn test_intersect_not_found_untracked_sends_done() {
    let prep = test_prep();
    let state = prep.state.clone();
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: Peer::new("peer1"),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::IntersectNotFound(Tip::origin()),
    });

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace(
        &running,
        &[
            te_state("tp-1", &state),
            te_input("tp-1", &msg),
            te_send("tp-1", &prep.handler, chainsync::InitiatorMessage::Done),
            te_state("tp-1", &state),
        ],
    );
    logs.assert_and_remove(Level::INFO, &["intersect not found"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

#[test]
fn test_intersect_not_found_removes_peer() {
    let prep = test_prep();
    let peer = Peer::new("peer1");
    let expected = prep.state.clone();
    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), Tip::origin(), Tip::origin());
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::IntersectNotFound(Tip::origin()),
    });

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace(
        &running,
        &[
            te_state("tp-1", &state),
            te_input("tp-1", &msg),
            te_send("tp-1", &prep.handler, chainsync::InitiatorMessage::Done),
            te_state("tp-1", &expected),
        ],
    );
    logs.assert_and_remove(Level::INFO, &["intersect not found"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

#[test]
fn test_roll_forward_unknown_peer_removes_peer() {
    let prep = test_prep();
    let state = prep.state.clone();
    let header = &prep.headers[0];
    let child = &prep.headers[1];
    let peer = Peer::new("peer1");
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(header, EraName::Conway), child.tip()),
    });

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace_contains(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::Adversarial(peer)).into(),
            te_state("tp-1", &state).into(),
        ],
    );
    logs.assert_and_remove(Level::ERROR, &["chain_sync.validate_header.failed", "Unknown peer"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_roll_forward_known_peer_header_already_stored() {
    let prep = test_prep();
    let peer = Peer::new("peer1");
    let parent = &prep.headers[0];
    let header = &prep.headers[1];
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(header, EraName::Conway), header.tip()),
    });

    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), parent.tip(), parent.tip());

    let mut expected = prep.state.clone();
    expected.insert_peer(peer.clone(), header.tip(), header.tip());

    let (running, _guards, mut logs) =
        setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(slice::from_ref(header)));
    assert_trace(
        &running,
        &[
            te_state("tp-1", &state),
            te_input("tp-1", &msg),
            te_send("tp-1", &prep.handler, RequestNext),
            te_clock_suspend("tp-1"),
            te_validate_header("tp-1", header.clone()),
            te_has_header("tp-1", header.hash()),
            te_state("tp-1", &expected),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["roll forward", "already stored"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

#[test]
fn test_roll_forward_known_peer_new_header_forwards_tip() {
    let prep = test_prep();
    let peer = Peer::new("peer1");
    let parent = &prep.headers[0];
    let header = &prep.headers[1];
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(header, EraName::Conway), header.tip()),
    });

    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), parent.tip(), parent.tip());

    let mut expected = prep.state.clone();
    expected.insert_peer(peer.clone(), header.tip(), header.tip());

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace(
        &running,
        &[
            te_state("tp-1", &state),
            te_input("tp-1", &msg),
            te_send("tp-1", &prep.handler, RequestNext),
            te_clock_suspend("tp-1"),
            te_validate_header("tp-1", header.clone()),
            te_has_header("tp-1", header.hash()),
            te_store_header("tp-1", header.clone()),
            te_send("tp-1", "downstream", (header.tip(), parent.point())),
            te_state("tp-1", &expected),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["roll forward", "new header"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

#[test]
fn test_roll_forward_invalid_variant_removes_peer() {
    let prep = test_prep();
    let peer = Peer::new("peer1");
    let parent = &prep.headers[0];
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::with_bytes(vec![], EraName::Babbage), parent.tip()),
    });

    let expected = prep.state.clone();
    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), parent.tip(), parent.tip());

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace(
        &running,
        &[
            te_state("tp-1", &state),
            te_input("tp-1", &msg),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::Adversarial(peer)),
            te_state("tp-1", &expected),
        ],
    );
    logs.assert_and_remove(Level::ERROR, &["chain_sync.decode_header.failed", "Invalid header variant"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_roll_forward_invalid_cbor_removes_peer() {
    let prep = test_prep();
    let peer = Peer::new("peer1");
    let parent = &prep.headers[0];
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(
            HeaderContent::with_bytes(vec![0xff], EraName::Conway),
            parent.tip(),
        ),
    });

    let expected = prep.state.clone();
    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), parent.tip(), parent.tip());

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace(
        &running,
        &[
            te_state("tp-1", &state),
            te_input("tp-1", &msg),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::Adversarial(peer)),
            te_state("tp-1", &expected),
        ],
    );
    logs.assert_and_remove(Level::ERROR, &["chain_sync.decode_header.failed", "Failed to decode header"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_roll_forward_invalid_parent_removes_peer() {
    let prep = test_prep();
    let peer = Peer::new("peer1");
    let parent = &prep.headers[0];
    let wrong_parent = HeaderHash::from([9u8; 32]);
    let header = make_block_header(2, 2, Some(wrong_parent));
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&header, EraName::Conway), header.tip()),
    });

    let expected = prep.state.clone();
    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), parent.tip(), parent.tip());

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace_contains(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::Adversarial(peer)).into(),
            te_state("tp-1", &expected).into(),
        ],
    );
    logs.assert_and_remove(Level::ERROR, &["chain_sync.validate_header.failed", "Invalid header parent"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_roll_forward_invalid_height_removes_peer() {
    let prep = test_prep();
    let peer = Peer::new("peer1");
    let parent = &prep.headers[0];
    let header = make_block_header(3, 2, Some(parent.hash()));
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&header, EraName::Conway), header.tip()),
    });

    let expected = prep.state.clone();
    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), parent.tip(), parent.tip());

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace_contains(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::Adversarial(peer)).into(),
            te_state("tp-1", &expected).into(),
        ],
    );
    logs.assert_and_remove(Level::ERROR, &["chain_sync.validate_header.failed", "Invalid header height"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_roll_forward_invalid_point_removes_peer() {
    let prep = test_prep();
    let peer = Peer::new("peer1");
    let parent = &prep.headers[0];
    let header = make_block_header(2, parent.slot().into(), Some(parent.hash()));
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&header, EraName::Conway), parent.tip()),
    });

    let expected = prep.state.clone();
    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), parent.tip(), parent.tip());

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace_contains(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::Adversarial(peer)).into(),
            te_state("tp-1", &expected).into(),
        ],
    );
    logs.assert_and_remove(Level::ERROR, &["chain_sync.validate_header.failed", "Invalid header point"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_roll_forward_header_validation_failure_removes_peer() {
    let prep = test_prep();
    let peer = Peer::new("peer1");
    let parent = &prep.headers[0];
    let header = &prep.headers[1];
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(header, EraName::Conway), header.tip()),
    });

    let expected = prep.state.clone();
    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), parent.tip(), header.tip());

    // Use empty store so evolve_nonce fails (unknown parent), exercising the real validate_header fn failure path.
    let (running, _guards, mut logs) =
        setup_base(&prep.rt_handle(), state.clone(), [msg.clone()], build_store(&[]), |running| {
            let header = header.hash();
            let parent = parent.hash();
            running.override_external_effect::<ValidateHeaderEffect>(usize::MAX, move |_| {
                OverrideResult::handled(Err(ValidateHeaderError::Nonces(NoncesError::UnknownParent { header, parent })))
            });
        });

    logs.assert_and_remove(Level::ERROR, &["chain_sync.validate_header.failed"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
    assert_trace(
        &running,
        &[
            te_state("tp-1", &state),
            te_input("tp-1", &msg),
            te_send("tp-1", &prep.handler, RequestNext),
            te_clock_suspend("tp-1"),
            te_validate_header("tp-1", header.clone()),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::Adversarial(peer)),
            te_state("tp-1", &expected),
        ],
    );
}

/// New test for header slot too far in future ( >2s according to clock ).
#[test]
fn test_roll_forward_header_slot_too_far_future_adversarial() {
    let prep = test_prep();
    let peer = Peer::new("peer1");
    let parent = &prep.headers[0];
    let header_slot = prep.start_at_slot + 10;
    let header = make_block_header(2, header_slot, Some(parent.hash()));
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&header, EraName::Conway), header.tip()),
    });

    let expected = prep.state.clone();
    let mut state = prep.state.clone();
    // make mono pass: current slot < header slot
    let curr_point = Point::Specific(1u64.into(), parent.hash());
    let curr_tip = Tip::new(curr_point, BlockHeight::from(1));
    state.insert_peer(peer.clone(), curr_tip, header.tip());

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));

    logs.assert_and_remove(Level::ERROR, &["chain_sync.validate_header.failed"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
    assert_trace_contains(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            te_clock_suspend("tp-1").into(),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::Adversarial(peer)).into(),
            te_state("tp-1", &expected).into(),
        ],
    );
}

/// New test for header slot near future (<=2s) -> defers validation.
#[test]
fn test_roll_forward_header_slot_near_future_defers() {
    let prep = test_prep();
    let peer = Peer::new("peer1");
    let parent = &prep.headers[0];
    // +1 s future from the effective ~1610 -> near, defer
    let header_slot = prep.start_at_slot + 2;
    let header = make_block_header(2, header_slot, Some(parent.hash()));
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&header, EraName::Conway), header.tip()),
    });

    let mut state = prep.state.clone();
    let curr_point = Point::Specific(1u64.into(), parent.hash());
    let curr_tip = Tip::new(curr_point, BlockHeight::from(1));
    state.insert_peer(peer.clone(), curr_tip, header.tip());

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));

    // no error log for near, defers instead of adversarial
    logs.assert_no_remaining_at([Level::ERROR]);
    assert_trace_contains(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            te_clock_suspend("tp-1").into(),
            // no adv send; defer happened (schedule + clockskew in deferred)
        ],
    );
    assert_trace_does_not_contain(&running, &[tm_send("tp-1", "peer_selection", PeerSelectionMsg::Adversarial(peer))]);
}

/// Tests that a header whose required stake distribution is more than 1 epoch ahead
/// causes immediate adversarial rejection (no deferral).
#[test]
fn test_roll_forward_stake_dist_far_ahead_rejects() {
    let prep = test_prep();
    let peer = Peer::new("peer1");
    let parent = &prep.headers[0];
    let header = &prep.headers[1];
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(header, EraName::Conway), header.tip()),
    });

    let expected = prep.state.clone();
    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), parent.tip(), header.tip());

    let far_epoch = Epoch::new(100);
    let slot = header.slot();
    // Override to simulate far-ahead stake dist not available (distance >1 -> reject)
    let (running, _guards, mut logs) =
        setup_base(&prep.rt_handle(), state.clone(), [msg.clone()], build_store(&[]), |running| {
            running.override_external_effect::<ValidateHeaderEffect>(usize::MAX, move |_| {
                OverrideResult::handled(Err(ValidateHeaderError::Assert(AssertHeaderError::PoolError(
                    GetPoolError::StakeDistributionNotAvailable(slot, Some(far_epoch)),
                ))))
            });
        });

    logs.assert_and_remove(Level::ERROR, &["chain_sync.validate_header.failed"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
    assert_trace_match(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            te_clock_suspend("tp-1").into(),
            te_validate_header("tp-1", header.clone()).into(),
            tm_volatile_tip("tp-1"),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::Adversarial(peer)).into(),
            te_state("tp-1", &expected).into(),
        ],
    );
}

#[test]
fn test_roll_backward_updates_peer() {
    let prep = test_prep();
    let peer = Peer::new("peer1");
    let header = &prep.headers[0];
    let current = header.point();
    let tip = Tip::new(current, BlockHeight::from(1));
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollBackward(current, tip),
    });

    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), Tip::origin(), Tip::origin());

    let mut expected = prep.state.clone();
    expected.insert_peer(peer, header.tip(), tip);

    let (running, _guards, mut logs) =
        setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(slice::from_ref(header)));
    assert_trace(
        &running,
        &[
            te_state("tp-1", &state),
            te_input("tp-1", &msg),
            te_send("tp-1", &prep.handler, RequestNext),
            te_load_tip("tp-1", current.hash()),
            te_state("tp-1", &expected),
        ],
    );
    logs.assert_and_remove(Level::INFO, &["roll backward"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

#[test]
fn test_roll_backward_unknown_peer_removes_peer() {
    let prep = test_prep();
    let peer = Peer::new("peer1");
    let header = &prep.headers[0];
    let current = header.point();
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollBackward(current, Tip::origin()),
    });

    let state = prep.state.clone();

    let (running, _guards, mut logs) =
        setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(slice::from_ref(header)));
    assert_trace(
        &running,
        &[
            te_state("tp-1", &state),
            te_input("tp-1", &msg),
            te_send("tp-1", &prep.handler, RequestNext),
            te_load_tip("tp-1", current.hash()),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::Adversarial(peer)),
            te_state("tp-1", &state),
        ],
    );
    logs.assert_and_remove(Level::ERROR, &["chain_sync.roll_backward.failed", "Unknown peer"])
        .assert_and_remove(Level::INFO, &["roll backward"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_roll_backward_unknown_point_removes_peer() {
    let prep = test_prep();
    let peer = Peer::new("peer1");
    let current = Point::Specific(1u64.into(), HeaderHash::from([1u8; 32]));
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollBackward(current, Tip::origin()),
    });

    let expected = prep.state.clone();
    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), Tip::origin(), Tip::origin());

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace(
        &running,
        &[
            te_state("tp-1", &state),
            te_input("tp-1", &msg),
            te_send("tp-1", &prep.handler, RequestNext),
            te_load_tip("tp-1", current.hash()),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::Adversarial(peer)),
            te_state("tp-1", &expected),
        ],
    );
    logs.assert_and_remove(Level::ERROR, &["chain_sync.roll_backward.failed", "Unknown point"])
        .assert_and_remove(Level::INFO, &["roll backward"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

/// Tests that a RollForward whose header height requires a ledger height beyond what is currently
/// applied causes scheduling of height recheck (self-message) for deferred headers
/// for a deferred RequestNext (instead of immediately pipelining RequestNext to the handler).
#[test]
fn test_roll_forward_defers_request_next() {
    // Use security_param = 0 so any header taller than the known ledger height triggers defer.
    let prep = test_prep_with_security_param(0);
    let peer = Peer::new("peer1");
    let header = prep.headers[0].clone();
    let tip = header.tip();

    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), Tip::origin(), tip);

    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&header, EraName::Conway), tip),
    });

    let store = build_store(&[]);

    // Use the special setup that forces ledger tip = origin (height 0).
    let (running, _guards, mut logs) =
        setup_with_ledger_tip(&prep.rt_handle(), state.clone(), msg.clone(), store, Tip::origin());

    logs.assert_and_remove(Level::DEBUG, &["track_peers.defer_request_next"]).assert_no_remaining_at([
        Level::ERROR,
        Level::WARN,
        Level::INFO,
    ]);

    assert_trace_contains(
        &running,
        &[
            tm_store_header("tp-1"),
            tm_state::<TrackPeers>("tp-1", |state| state.deferred.len() == 1, ""),
            tm_clock(Duration::from_secs(1654041610) + Duration::from_millis(200)),
        ],
    );

    // The handler must *not* have received an immediate RequestNext (that is the whole point of deferring).
    assert_trace_does_not_contain(&running, &[tm_send("tp-1", "", InitiatorMessage::RequestNext)]);
}

#[test]
fn test_pipelined_headers_after_height_defer() {
    let prep = test_prep_with_security_param(0);
    let peer = Peer::new("peer1");
    let parent = &prep.headers[0];
    let h1 = prep.headers[1].clone();
    let h2 = make_block_header(3, h1.slot().as_u64() + 1, Some(h1.hash()));

    let msg1 = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&h1, EraName::Conway), h1.tip()),
    });
    let msg2 = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&h2, EraName::Conway), h2.tip()),
    });

    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), parent.tip(), h2.tip());

    // Use setup_base (now accepts multiple) with forced ledger tip at origin so height defers apply.
    let store = build_store(&[]);
    let (running, _guards, _logs) =
        setup_base(&prep.rt_handle(), state.clone(), [msg1.clone(), msg2.clone()], store, |running| {
            running.override_external_effect::<VolatileTipEffect>(usize::MAX, {
                let tip = Tip::origin();
                move |_| OverrideResult::handled(tip)
            });
            running.override_external_effect::<TipEffect>(usize::MAX, {
                let tip = Tip::origin();
                move |_| OverrideResult::handled(tip)
            });
            // Default validate succeeds (height defer happens before full validate in this path)
            running.override_external_effect::<ValidateHeaderEffect>(usize::MAX, |_| OverrideResult::handled(Ok(())));
        });

    // Locked down trace for pipelined height defer (both headers processed, RN defers queued for sequence).
    assert_trace_contains(
        &running,
        &[
            te_input("tp-1", &msg1).into(),
            te_validate_header("tp-1", h1.clone()).into(),
            te_store_header("tp-1", h1.clone()).into(),
            tm_state::<TrackPeers>("tp-1", |s| s.deferred.len() == 1, "one RN defer queued"),
            te_input("tp-1", &msg2).into(),
            te_validate_header("tp-1", h2.clone()).into(),
            te_store_header("tp-1", h2.clone()).into(),
            tm_state::<TrackPeers>("tp-1", |s| s.deferred.len() == 2, "two RN defers queued for pipelined"),
        ],
    );
}

#[test]
fn test_pipelined_headers_after_slot_near_future_defer() {
    let prep = test_prep();
    let peer = Peer::new("peer1");
    let parent = &prep.headers[0];
    // +1s future -> near future defer for first
    let h1 = make_block_header(2, prep.start_at_slot + 1, Some(parent.hash()));
    let h2 = make_block_header(3, prep.start_at_slot + 2, Some(h1.hash()));

    let msg1 = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&h1, EraName::Conway), h1.tip()),
    });
    let msg2 = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&h2, EraName::Conway), h2.tip()),
    });

    let mut state = prep.state.clone();
    let curr_point = Point::Specific(1u64.into(), parent.hash());
    let curr_tip = Tip::new(curr_point, BlockHeight::from(1));
    state.insert_peer(peer.clone(), curr_tip, h2.tip());

    let (running, _guards, _logs) =
        setup_base(&prep.rt_handle(), state.clone(), [msg1.clone(), msg2.clone()], build_store(&[]), |running| {
            running.override_external_effect::<ValidateHeaderEffect>(usize::MAX, |_| OverrideResult::handled(Ok(())));
        });

    // no adversarial expected in defer case
    assert_trace_does_not_contain(
        &running,
        &[tm_send("tp-1", "peer_selection", PeerSelectionMsg::Adversarial(peer.clone()))],
    );
    // Locked down: no adversarial for pipelined near-future slot defer case.
    // (trace contents for this slot calc are minimal in harness; core no-adv verified)
    assert_trace_does_not_contain(
        &running,
        &[tm_send("tp-1", "peer_selection", PeerSelectionMsg::Adversarial(peer.clone()))],
    );
}

/// Pipelined stake dist not available: multiple headers arrive (from pipelining), both defer,
/// then StakeDistUpdated wakes them for sequential re-validation and processing.
#[test]
fn test_pipelined_stake_defer_and_wake_sequence() {
    let prep = test_prep();
    let peer = Peer::new("peer1");
    let parent = &prep.headers[0];
    let h1 = prep.headers[1].clone();
    let h2 = make_block_header(3, h1.slot().as_u64() + 1, Some(h1.hash()));

    let msg1 = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&h1, EraName::Conway), h1.tip()),
    });
    let msg2 = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&h2, EraName::Conway), h2.tip()),
    });
    let wake = TrackPeersMsg::StakeDistUpdated;

    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), parent.tip(), h2.tip());

    let slot1 = h1.slot();
    let _slot2 = h2.slot();
    // target epoch chosen so dist <= curr +1 to trigger defer not reject
    let target_epoch = Epoch::new(0);

    let call_count = Arc::new(AtomicUsize::new(0));
    let (running, _guards, _logs) = setup_base(
        &prep.rt_handle(),
        state.clone(),
        [msg1.clone(), msg2.clone(), wake.clone()],
        build_store(&[]),
        |running| {
            let cc = call_count.clone();
            running.override_external_effect::<ValidateHeaderEffect>(usize::MAX, move |_| {
                let c = cc.fetch_add(1, Ordering::SeqCst);
                if c < 2 {
                    OverrideResult::handled(Err(ValidateHeaderError::Assert(AssertHeaderError::PoolError(
                        GetPoolError::StakeDistributionNotAvailable(slot1, Some(target_epoch)),
                    ))))
                } else {
                    OverrideResult::handled(Ok(()))
                }
            });
        },
    );

    // Locked down: pipelined stake defers (RN sent early for both), both queued, then on wake re-validated and stored in sequence, deferred cleared.
    assert_trace_contains(
        &running,
        &[
            te_input("tp-1", &msg1).into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            te_validate_header("tp-1", h1.clone()).into(),
            tm_state::<TrackPeers>("tp-1", |s| s.deferred.len() == 1, "first stake deferred"),
            te_input("tp-1", &msg2).into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            te_validate_header("tp-1", h2.clone()).into(),
            tm_state::<TrackPeers>("tp-1", |s| s.deferred.len() == 2, "second pipelined stake deferred"),
            te_input("tp-1", &wake).into(),
            te_validate_header("tp-1", h1.clone()).into(),
            te_has_header("tp-1", h1.hash()).into(),
            te_store_header("tp-1", h1.clone()).into(),
            te_validate_header("tp-1", h2.clone()).into(),
            te_has_header("tp-1", h2.hash()).into(),
            te_store_header("tp-1", h2.clone()).into(),
            tm_state::<TrackPeers>(
                "tp-1",
                |s| {
                    s.deferred.is_empty() && s.upstream.get(&peer).is_some_and(|p| p.current.block_height() == 3.into())
                },
                "both processed after wake",
            ),
        ],
    );
}
