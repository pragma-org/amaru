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

use std::{slice, sync::Arc};

use amaru_kernel::{BlockHeight, EraName, HeaderHash, IsHeader, Peer, Point, Tip};
use amaru_protocols::chainsync::{
    self, ChainSyncInitiatorMsg, HeaderContent, InitiatorMessage, InitiatorMessage::RequestNext,
};
use pure_stage::{
    assert_trace_contains, assert_trace_does_not_contain, tm_add_stage, tm_send,
    trace_match::{tm_send_type, tm_wire_stage_state},
};
use tracing::Level;

use crate::stages::{
    peer_selection::PeerSelectionMsg,
    test_utils::{assert_trace, te_input, te_send, te_state, tm_state},
    track_peers::{
        DeferReqNextMsg, TrackPeers, TrackPeersMsg,
        defer_req_next::DeferReqNext,
        test_setup::{
            FailingHeaderValidation, build_store, make_block_header, setup, setup_with_ledger_tip,
            setup_with_validation, te_has_header, te_load_tip, te_store_header, te_validate_header, test_prep,
            test_prep_with_security_param, tm_store_header,
        },
    },
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
    assert_trace(
        &running,
        &[
            te_state("tp-1", &state),
            te_input("tp-1", &msg),
            te_send("tp-1", &prep.handler, RequestNext),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::Adversarial(peer)),
            te_state("tp-1", &state),
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
    assert_trace(
        &running,
        &[
            te_state("tp-1", &state),
            te_input("tp-1", &msg),
            te_send("tp-1", &prep.handler, RequestNext),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::Adversarial(peer)),
            te_state("tp-1", &expected),
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
    assert_trace(
        &running,
        &[
            te_state("tp-1", &state),
            te_input("tp-1", &msg),
            te_send("tp-1", &prep.handler, RequestNext),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::Adversarial(peer)),
            te_state("tp-1", &expected),
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
    assert_trace(
        &running,
        &[
            te_state("tp-1", &state),
            te_input("tp-1", &msg),
            te_send("tp-1", &prep.handler, RequestNext),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::Adversarial(peer)),
            te_state("tp-1", &expected),
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

    let (running, _guards, mut logs) = setup_with_validation(
        &prep.rt_handle(),
        state.clone(),
        msg.clone(),
        build_store(&[]),
        Arc::new(FailingHeaderValidation),
    );

    logs.assert_and_remove(Level::ERROR, &["chain_sync.validate_header.failed", "booyah!"]).assert_no_remaining_at([
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
            te_validate_header("tp-1", header.clone()),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::Adversarial(peer)),
            te_state("tp-1", &expected),
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
/// applied causes the stage to create the `defer_req_next` child stage and register the handler
/// for a deferred RequestNext (instead of immediately pipelining RequestNext to the handler).
#[test]
fn test_roll_forward_defers_request_next_and_creates_defer_child() {
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
        setup_with_ledger_tip(&prep.rt_handle(), state, msg.clone(), store, Tip::origin());

    logs.assert_and_remove(Level::DEBUG, &["track_peers.defer_request_next"]).assert_no_remaining_at([
        Level::ERROR,
        Level::WARN,
        Level::INFO,
    ]);

    // Use the new subsequence matcher + property-based TraceMatch for the dynamically named child.
    // We assert that the following events appear in this order (with other unrelated entries allowed in between).
    assert_trace_contains(
        &running,
        &[
            tm_store_header("tp-1"),
            tm_add_stage("tp-1", "track_peers/defer_req_next"),
            tm_wire_stage_state("tp-1", "defer_req_next", DeferReqNext::new(200)),
            tm_send("tp-1", "defer_req_next", DeferReqNextMsg::Poll),
            tm_send_type::<DeferReqNextMsg>("tp-1", "defer_req_next"),
            tm_state(
                "tp-1",
                |s: &TrackPeers| s.defer_req_next.name().as_str().contains("defer_req_next"),
                "stored defer_req_next ref",
            ),
        ],
    );

    // The handler must *not* have received an immediate RequestNext (that is the whole point of deferring).
    assert_trace_does_not_contain(&running, &[tm_send("tp-1", "", InitiatorMessage::RequestNext)]);
}
