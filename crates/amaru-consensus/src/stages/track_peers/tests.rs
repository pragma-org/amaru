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

use std::{self, slice, time::Duration};

use amaru_kernel::{BlockHeight, Epoch, EraHistory, EraName, HeaderHash, IsHeader, Peer, Point, Tip, num::CheckedSub};
use amaru_ouroboros::{ConnectionId, praos::header::AssertHeaderError};
use amaru_ouroboros_traits::has_stake_distribution::GetPoolError;
use amaru_protocols::chainsync::{
    self, ChainSyncInitiatorMsg, HeaderContent, InitiatorMessage, InitiatorMessage::RequestNext,
};
use amaru_pure_stage::{
    assert_trace_contains, assert_trace_does_not_contain, assert_trace_match, simulation::running::OverrideResult,
    tm_send,
};
use tracing::Level;

use crate::{
    effects::{ValidateHeaderEffect, VolatileTipEffect},
    stages::{
        peer_selection::PeerSelectionMsg,
        test_utils::{assert_trace, te_input, te_send, te_state, tm_state},
        track_peers::{
            TrackPeers, TrackPeersMsg,
            test_setup::{
                HEIGHT_RECHECK_INTERVAL, build_store, height_recheck_schedule_id, make_block_header, new_tip,
                schedule_id_at, setup, setup_base, setup_with_ledger_tip_until_sleeping, te_clock, te_clock_suspend,
                te_has_header, te_load_tip, te_schedule, te_store_header, te_validate_header, test_prep,
                test_prep_with_max_peer_lead, tm_volatile_tip,
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
    let peer = Peer::new("peer1");
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::Initialize,
    });

    let mut expected = state.clone();
    expected.record_connecting(peer, prep.conn_id);

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace(&running, &[te_state("tp-1", &state), te_input("tp-1", &msg), te_state("tp-1", &expected)]);
    logs.assert_and_remove(Level::INFO, &["initializing chainsync"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

#[test]
fn test_initialize_resets_established_session() {
    let prep = test_prep();
    let peer = Peer::new("peer1");
    let header = &prep.headers[1];
    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), prep.conn_id, Tip::origin(), Tip::origin());
    state.push_deferred_for_tests(peer.clone(), prep.conn_id, prep.handler.clone(), header.clone(), header.tip());
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::Initialize,
    });

    let mut expected = prep.state.clone();
    expected.record_connecting(peer, prep.conn_id);

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace(&running, &[te_state("tp-1", &state), te_input("tp-1", &msg), te_state("tp-1", &expected)]);
    logs.assert_and_remove(Level::WARN, &["unexpected re-initialize"])
        .assert_and_remove(Level::INFO, &["initializing chainsync"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_terminated_purges_upstream_and_deferred() {
    let prep = test_prep_with_max_peer_lead(0);
    let peer = Peer::new("peer1");
    let parent = &prep.headers[0];
    let header = &prep.headers[1];
    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), prep.conn_id, parent.tip(), parent.tip());
    state.push_deferred_for_tests(peer.clone(), prep.conn_id, prep.handler.clone(), header.clone(), header.tip());

    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::Terminated,
    });

    let expected = prep.state.clone(); // empty upstream + deferred

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace(&running, &[te_state("tp-1", &state), te_input("tp-1", &msg), te_state("tp-1", &expected)]);
    logs.assert_and_remove(Level::INFO, &["chainsync terminated"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

#[test]
fn test_terminated_only_purges_matching_connection() {
    let prep = test_prep();
    let peer = Peer::new("peer1");
    let mut other_id = ConnectionId::initial();
    let conn_a = other_id.get_and_increment();
    let conn_b = other_id.get_and_increment();

    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), conn_a, Tip::origin(), Tip::origin());
    state.insert_peer(peer.clone(), conn_b, prep.headers[0].tip(), prep.headers[0].tip());

    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: conn_a,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::Terminated,
    });

    let mut expected = prep.state.clone();
    expected.insert_peer(peer, conn_b, prep.headers[0].tip(), prep.headers[0].tip());

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace(&running, &[te_state("tp-1", &state), te_input("tp-1", &msg), te_state("tp-1", &expected)]);
    logs.assert_and_remove(Level::INFO, &["chainsync terminated"]).assert_no_remaining_at([
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
    expected.insert_peer(Peer::new("peer1"), prep.conn_id, header.tip(), tip);

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
    state.insert_peer(peer.clone(), prep.conn_id, Tip::origin(), Tip::origin());
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
    assert_trace_match(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_clock_suspend("tp-1").into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer)).into(),
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
    state.insert_peer(peer.clone(), prep.conn_id, parent.tip(), parent.tip());

    let mut expected = prep.state.clone();
    expected.insert_peer(peer.clone(), prep.conn_id, header.tip(), header.tip());

    let (running, _guards, mut logs) =
        setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(slice::from_ref(header)));
    assert_trace_match(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_clock_suspend("tp-1").into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            te_validate_header("tp-1", header.clone()).into(),
            te_has_header("tp-1", header.hash()).into(),
            te_state("tp-1", &expected).into(),
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
    state.insert_peer(peer.clone(), prep.conn_id, parent.tip(), parent.tip());

    let mut expected = prep.state.clone();
    expected.insert_peer(peer.clone(), prep.conn_id, header.tip(), header.tip());

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace_match(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_clock_suspend("tp-1").into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            te_validate_header("tp-1", header.clone()).into(),
            te_has_header("tp-1", header.hash()).into(),
            te_store_header("tp-1", header.clone()).into(),
            te_send("tp-1", "downstream", new_tip(header.tip(), parent.point())).into(),
            te_state("tp-1", &expected).into(),
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
    state.insert_peer(peer.clone(), prep.conn_id, parent.tip(), parent.tip());

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace(
        &running,
        &[
            te_state("tp-1", &state),
            te_input("tp-1", &msg),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer)),
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
    state.insert_peer(peer.clone(), prep.conn_id, parent.tip(), parent.tip());

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace(
        &running,
        &[
            te_state("tp-1", &state),
            te_input("tp-1", &msg),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer)),
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
    state.insert_peer(peer.clone(), prep.conn_id, parent.tip(), parent.tip());

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace_match(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_clock_suspend("tp-1").into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer)).into(),
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
    state.insert_peer(peer.clone(), prep.conn_id, parent.tip(), parent.tip());

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace_match(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_clock_suspend("tp-1").into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer)).into(),
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
    state.insert_peer(peer.clone(), prep.conn_id, parent.tip(), parent.tip());

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace_match(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_clock_suspend("tp-1").into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer)).into(),
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
    state.insert_peer(peer.clone(), prep.conn_id, parent.tip(), header.tip());

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
    assert_trace_match(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_clock_suspend("tp-1").into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            te_validate_header("tp-1", header.clone()).into(),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer)).into(),
            te_state("tp-1", &expected).into(),
        ],
    );
}

/// Header slot more than 2 slots ahead of sim clock → adversarial.
/// Slot math must use the same [`EraHistory`] as `TrackPeers` (`EraHistory::default()` in tests).
#[test]
fn test_roll_forward_header_slot_too_far_future_adversarial() {
    let prep = test_prep();
    let peer = Peer::new("peer1");
    let parent = &prep.headers[0];
    let elapsed = prep.start_times.relative_time + Duration::from_secs(10);
    let curr_slot = EraHistory::default().relative_time_to_slot(elapsed).expect("slot from start time").as_u64();
    let header = make_block_header(2, curr_slot + 10, Some(parent.hash()));
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&header, EraName::Conway), header.tip()),
    });

    let expected = prep.state.clone();
    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), prep.conn_id, parent.tip(), header.tip());

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));

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
            te_clock_suspend("tp-1").into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer)).into(),
            te_state("tp-1", &expected).into(),
        ],
    );
}

/// Header slot 1–2 ahead of sim clock → clock-skew defer (not adversarial).
#[test]
fn test_roll_forward_header_slot_near_future_defers() {
    let prep = test_prep();
    let peer = Peer::new("peer1");
    let parent = &prep.headers[0];
    let elapsed = prep.start_times.relative_time + Duration::from_secs(10);
    let curr_slot = EraHistory::default().relative_time_to_slot(elapsed).expect("slot from start time").as_u64();
    // one slot ahead of sim clock → near-future defer
    let header = make_block_header(2, curr_slot + 1, Some(parent.hash()));
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&header, EraName::Conway), header.tip()),
    });

    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), prep.conn_id, parent.tip(), header.tip());

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));

    logs.assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
    // Clock-skew defers, then sim advances and RecheckLedgerHeight processes the header.
    assert_trace_contains(
        &running,
        &[
            te_input("tp-1", &msg).into(),
            te_clock_suspend("tp-1").into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            te_clock_suspend("tp-1").into(),
            tm_state::<TrackPeers>("tp-1", |s| s.deferred.len() == 1, "clock skew deferred"),
            te_input("tp-1", &TrackPeersMsg::RecheckLedgerHeight).into(),
            te_validate_header("tp-1", header.clone()).into(),
            te_store_header("tp-1", header.clone()).into(),
            tm_state::<TrackPeers>("tp-1", |s| s.deferred.is_empty(), "processed after recheck"),
        ],
    );
    assert_trace_does_not_contain(&running, &[tm_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer))]);
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
    state.insert_peer(peer.clone(), prep.conn_id, parent.tip(), header.tip());

    // More than one epoch beyond known max_epoch (start-2) → adversarial, not defer.
    let far_epoch = prep.start_times.epoch;
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
            te_clock_suspend("tp-1").into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            te_validate_header("tp-1", header.clone()).into(),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer)).into(),
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
    state.insert_peer(peer.clone(), prep.conn_id, Tip::origin(), Tip::origin());

    let mut expected = prep.state.clone();
    expected.insert_peer(peer, prep.conn_id, header.tip(), tip);

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
            te_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer)),
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
    state.insert_peer(peer.clone(), prep.conn_id, Tip::origin(), Tip::origin());

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace(
        &running,
        &[
            te_state("tp-1", &state),
            te_input("tp-1", &msg),
            te_send("tp-1", &prep.handler, RequestNext),
            te_load_tip("tp-1", current.hash()),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer)),
            te_state("tp-1", &expected),
        ],
    );
    logs.assert_and_remove(Level::ERROR, &["chain_sync.roll_backward.failed", "Unknown point"])
        .assert_and_remove(Level::INFO, &["roll backward"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

/// Tests that a RollForward whose header height requires a ledger height beyond what is currently
/// applied defers RequestNext and arms a single coalesced height-recheck schedule.
#[test]
fn test_roll_forward_defers_request_next() {
    // Use max_peer_lead = 0 so any header taller than the known ledger height triggers defer.
    let prep = test_prep_with_max_peer_lead(0);
    let peer = Peer::new("peer1");
    let header = prep.headers[0].clone();
    let tip = header.tip();

    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), prep.conn_id, Tip::origin(), tip);

    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&header, EraName::Conway), tip),
    });

    let store = build_store(&[]);
    let sid = height_recheck_schedule_id();

    // Frozen ledger tip would poll forever if wakeups auto-advanced; stop at first sleep.
    let (running, _guards, mut logs) =
        setup_with_ledger_tip_until_sleeping(&prep.rt_handle(), state.clone(), [msg.clone()], store, Tip::origin());

    logs.assert_and_remove(Level::DEBUG, &["track_peers.defer_request_next"]).assert_no_remaining_at([
        Level::ERROR,
        Level::WARN,
        Level::INFO,
    ]);

    assert_trace_match(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_clock_suspend("tp-1").into(),
            tm_volatile_tip("tp-1"),
            te_clock_suspend("tp-1").into(),
            te_schedule("tp-1", TrackPeersMsg::RecheckLedgerHeight, sid).into(),
            tm_state::<TrackPeers>(
                "tp-1",
                |s| s.deferred.len() == 1 && s.recheck_timer == Some(sid),
                "ledger height deferred with recheck armed",
            ),
        ],
    );

    // The handler must *not* have received an immediate RequestNext (that is the whole point of deferring).
    assert_trace_does_not_contain(&running, &[tm_send("tp-1", "", InitiatorMessage::RequestNext)]);
}

#[test]
fn test_pipelined_headers_after_height_defer() {
    let prep = test_prep_with_max_peer_lead(0);
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
    state.insert_peer(peer.clone(), prep.conn_id, parent.tip(), h2.tip());

    let sid = height_recheck_schedule_id();

    // Forced ledger tip = origin so height defers apply; second header is FollowUp while peer deferred.
    // Stop at first sleep so the height-poll loop does not run forever under a frozen tip.
    let (running, _guards, mut logs) = setup_with_ledger_tip_until_sleeping(
        &prep.rt_handle(),
        state.clone(),
        [msg1.clone(), msg2.clone()],
        build_store(&[]),
        Tip::origin(),
    );

    logs.assert_and_remove(Level::DEBUG, &["track_peers.defer_request_next"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
    assert_trace_match(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg1).into(),
            te_clock_suspend("tp-1").into(),
            tm_volatile_tip("tp-1"),
            te_clock_suspend("tp-1").into(),
            te_schedule("tp-1", TrackPeersMsg::RecheckLedgerHeight, sid).into(),
            tm_state::<TrackPeers>("tp-1", |s| s.deferred.len() == 1, "first ledger-height deferred"),
            te_input("tp-1", &msg2).into(),
            tm_state::<TrackPeers>(
                "tp-1",
                |s| s.deferred.len() == 2 && s.recheck_timer == Some(sid),
                "follow-up queued while deferred; still one recheck timer",
            ),
        ],
    );
    assert_trace_does_not_contain(&running, &[tm_send("tp-1", "", InitiatorMessage::RequestNext)]);
}

/// Height defer is released when a later recheck sees the applied ledger height advance.
#[test]
fn test_height_defer_recheck_when_ledger_advances() {
    let prep = test_prep_with_max_peer_lead(0);
    let peer = Peer::new("peer1");
    let header = prep.headers[0].clone();
    let tip = header.tip();

    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), prep.conn_id, Tip::origin(), tip);

    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&header, EraName::Conway), tip),
    });

    let sid = height_recheck_schedule_id();
    let recheck_at = schedule_id_at(HEIGHT_RECHECK_INTERVAL).time();
    let advanced_tip = Tip::new(header.point(), header.block_height());

    let (running, _guards, mut logs) =
        setup_base(&prep.rt_handle(), state.clone(), [msg.clone()], build_store(&[]), |running| {
            let mut n = 0u8;
            running.override_external_effect::<VolatileTipEffect>(usize::MAX, move |_| {
                n += 1;
                // First call (defer decision) still at origin; recheck sees advanced height.
                if n == 1 { OverrideResult::handled(Tip::origin()) } else { OverrideResult::handled(advanced_tip) }
            });
            running.override_external_effect::<ValidateHeaderEffect>(usize::MAX, |_| OverrideResult::handled(Ok(())));
        });

    logs.assert_and_remove(Level::DEBUG, &["track_peers.defer_request_next"])
        .assert_and_remove(Level::DEBUG, &["roll forward"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);

    assert_trace_match(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_clock_suspend("tp-1").into(),
            tm_volatile_tip("tp-1"),
            te_clock_suspend("tp-1").into(),
            te_schedule("tp-1", TrackPeersMsg::RecheckLedgerHeight, sid).into(),
            tm_state::<TrackPeers>("tp-1", |s| s.deferred.len() == 1, "height deferred"),
            te_clock(recheck_at).into(),
            te_input("tp-1", &TrackPeersMsg::RecheckLedgerHeight).into(),
            tm_volatile_tip("tp-1"),
            te_clock_suspend("tp-1").into(),
            te_validate_header("tp-1", header.clone()).into(),
            te_has_header("tp-1", header.hash()).into(),
            te_store_header("tp-1", header.clone()).into(),
            te_send("tp-1", "downstream", new_tip(header.tip(), Point::Origin)).into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            tm_state::<TrackPeers>(
                "tp-1",
                |s| s.deferred.is_empty() && s.recheck_timer.is_none(),
                "processed after height advanced",
            ),
        ],
    );
}

#[test]
fn test_pipelined_headers_after_slot_near_future_defer() {
    let prep = test_prep();
    let peer = Peer::new("peer1");
    let parent = &prep.headers[0];
    let elapsed = prep.start_times.relative_time + Duration::from_secs(10);
    let curr_slot = EraHistory::default().relative_time_to_slot(elapsed).expect("slot from start time").as_u64();
    // first is near-future; second is FollowUp while peer deferred
    let h1 = make_block_header(2, curr_slot + 1, Some(parent.hash()));
    let h2 = make_block_header(3, curr_slot + 2, Some(h1.hash()));

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
    state.insert_peer(peer.clone(), prep.conn_id, parent.tip(), h2.tip());

    let (running, _guards, mut logs) =
        setup_base(&prep.rt_handle(), state.clone(), [msg1.clone(), msg2.clone()], build_store(&[]), |running| {
            running.override_external_effect::<ValidateHeaderEffect>(usize::MAX, |_| OverrideResult::handled(Ok(())));
        });

    logs.assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
    // First header clock-skew defers; second is FollowUp; recheck may drain both before run ends.
    assert_trace_contains(
        &running,
        &[
            te_input("tp-1", &msg1).into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            tm_state::<TrackPeers>("tp-1", |s| s.deferred.len() == 1, "first clock-skew deferred"),
            te_input("tp-1", &msg2).into(),
            tm_state::<TrackPeers>("tp-1", |s| s.deferred.len() == 2, "follow-up queued while deferred"),
        ],
    );
    assert_trace_does_not_contain(&running, &[tm_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer))]);
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
    // Advance max_epoch far enough that the previously missing target epoch is covered.
    let wake = TrackPeersMsg::StakeDistUpdated(prep.start_times.epoch);

    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), prep.conn_id, parent.tip(), h2.tip());

    let slot1 = h1.slot();
    // One epoch ahead of known max_epoch (start-2) → defer, not reject.
    let target_epoch = prep.start_times.epoch.checked_sub(Epoch::ONE).unwrap();

    let (running, _guards, mut logs) = setup_base(
        &prep.rt_handle(),
        state.clone(),
        [msg1.clone(), msg2.clone(), wake.clone()],
        // Parent header present so recheck nonce evolution can succeed if real validation runs.
        build_store(slice::from_ref(parent)),
        |running| {
            // First validate fails (missing stake); later calls succeed.
            let mut n = 0u8;
            running.override_external_effect::<ValidateHeaderEffect>(usize::MAX, move |_| {
                n += 1;
                if n == 1 {
                    OverrideResult::handled(Err(ValidateHeaderError::Assert(AssertHeaderError::PoolError(
                        GetPoolError::StakeDistributionNotAvailable(slot1, Some(target_epoch)),
                    ))))
                } else {
                    OverrideResult::handled(Ok(()))
                }
            });
        },
    );

    logs.assert_and_remove(Level::DEBUG, &["roll forward"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
    // h1 stake-deferred after RN; h2 is FollowUp (peer already deferred); wake reprocesses both in order.
    assert_trace_match(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg1).into(),
            te_clock_suspend("tp-1").into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            te_validate_header("tp-1", h1.clone()).into(),
            tm_state::<TrackPeers>("tp-1", |s| s.deferred.len() == 1, "first stake deferred"),
            te_input("tp-1", &msg2).into(),
            tm_state::<TrackPeers>("tp-1", |s| s.deferred.len() == 2, "follow-up queued"),
            te_input("tp-1", &wake).into(),
            tm_volatile_tip("tp-1"),
            te_clock_suspend("tp-1").into(),
            te_validate_header("tp-1", h1.clone()).into(),
            te_has_header("tp-1", h1.hash()).into(),
            te_store_header("tp-1", h1.clone()).into(),
            te_send("tp-1", "downstream", new_tip(h1.tip(), parent.point())).into(),
            te_validate_header("tp-1", h2.clone()).into(),
            te_has_header("tp-1", h2.hash()).into(),
            te_store_header("tp-1", h2.clone()).into(),
            te_send("tp-1", "downstream", new_tip(h2.tip(), h1.point())).into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            tm_state::<TrackPeers>(
                "tp-1",
                |s| {
                    s.deferred.is_empty()
                        && s.recheck_timer.is_none()
                        && s.upstream.get(&prep.conn_id).is_some_and(|p| {
                            p.established().is_some_and(|(current, _)| current.block_height() == 3.into())
                        })
                },
                "both processed after wake",
            ),
        ],
    );
}
