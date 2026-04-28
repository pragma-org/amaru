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
use amaru_protocols::{
    chainsync::{self, ChainSyncInitiatorMsg, HeaderContent, InitiatorMessage::RequestNext},
    manager::ManagerMessage,
};
use tracing::Level;

use crate::stages::{
    test_utils::{assert_trace, tm_input, tm_send, tm_state},
    track_peers::{
        TrackPeersMsg,
        test_setup::{
            FailingHeaderValidation, build_store, make_block_header, setup, setup_with_validation, test_prep,
            tm_has_header, tm_load_header, tm_store_header, tm_validate_header,
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
    assert_trace(&running, &[tm_state("tp-1", &state.clone()), tm_input("tp-1", &msg), tm_state("tp-1", &state)]);
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
    assert_trace(&running, &[tm_state("tp-1", &state), tm_input("tp-1", &msg), tm_state("tp-1", &state)]);
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
            tm_state("tp-1", &state.clone()),
            tm_input("tp-1", &msg),
            tm_load_header("tp-1", current.hash()),
            tm_send("tp-1", &prep.handler, chainsync::InitiatorMessage::Done),
            tm_state("tp-1", &state),
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
            tm_state("tp-1", &state),
            tm_input("tp-1", &msg),
            tm_load_header("tp-1", current.hash()),
            tm_state("tp-1", &expected),
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
            tm_state("tp-1", &state.clone()),
            tm_input("tp-1", &msg),
            tm_send("tp-1", &prep.handler, chainsync::InitiatorMessage::Done),
            tm_state("tp-1", &state),
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
            tm_state("tp-1", &state),
            tm_input("tp-1", &msg),
            tm_send("tp-1", &prep.handler, chainsync::InitiatorMessage::Done),
            tm_state("tp-1", &expected),
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
            tm_state("tp-1", &state.clone()),
            tm_input("tp-1", &msg),
            tm_send("tp-1", &prep.handler, RequestNext),
            tm_send("tp-1", "manager", ManagerMessage::RemovePeer(peer)),
            tm_state("tp-1", &state),
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
            tm_state("tp-1", &state),
            tm_input("tp-1", &msg),
            tm_send("tp-1", &prep.handler, RequestNext),
            tm_validate_header("tp-1", header.clone()),
            tm_has_header("tp-1", header.hash()),
            tm_state("tp-1", &expected),
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
            tm_state("tp-1", &state),
            tm_input("tp-1", &msg),
            tm_send("tp-1", &prep.handler, RequestNext),
            tm_validate_header("tp-1", header.clone()),
            tm_has_header("tp-1", header.hash()),
            tm_store_header("tp-1", header.clone()),
            tm_send("tp-1", "downstream", (header.tip(), parent.point())),
            tm_state("tp-1", &expected),
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
            tm_state("tp-1", &state),
            tm_input("tp-1", &msg),
            tm_send("tp-1", "manager", ManagerMessage::RemovePeer(peer)),
            tm_state("tp-1", &expected),
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
            tm_state("tp-1", &state),
            tm_input("tp-1", &msg),
            tm_send("tp-1", "manager", ManagerMessage::RemovePeer(peer)),
            tm_state("tp-1", &expected),
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
            tm_state("tp-1", &state),
            tm_input("tp-1", &msg),
            tm_send("tp-1", &prep.handler, RequestNext),
            tm_send("tp-1", "manager", ManagerMessage::RemovePeer(peer)),
            tm_state("tp-1", &expected),
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
            tm_state("tp-1", &state),
            tm_input("tp-1", &msg),
            tm_send("tp-1", &prep.handler, RequestNext),
            tm_send("tp-1", "manager", ManagerMessage::RemovePeer(peer)),
            tm_state("tp-1", &expected),
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
            tm_state("tp-1", &state),
            tm_input("tp-1", &msg),
            tm_send("tp-1", &prep.handler, RequestNext),
            tm_send("tp-1", "manager", ManagerMessage::RemovePeer(peer)),
            tm_state("tp-1", &expected),
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
            tm_state("tp-1", &state),
            tm_input("tp-1", &msg),
            tm_send("tp-1", &prep.handler, RequestNext),
            tm_validate_header("tp-1", header.clone()),
            tm_send("tp-1", "manager", ManagerMessage::RemovePeer(peer)),
            tm_state("tp-1", &expected),
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
            tm_state("tp-1", &state),
            tm_input("tp-1", &msg),
            tm_send("tp-1", &prep.handler, RequestNext),
            tm_load_header("tp-1", current.hash()),
            tm_state("tp-1", &expected),
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
            tm_state("tp-1", &state.clone()),
            tm_input("tp-1", &msg),
            tm_send("tp-1", &prep.handler, RequestNext),
            tm_load_header("tp-1", current.hash()),
            tm_send("tp-1", "manager", ManagerMessage::RemovePeer(peer)),
            tm_state("tp-1", &state),
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
            tm_state("tp-1", &state),
            tm_input("tp-1", &msg),
            tm_send("tp-1", &prep.handler, RequestNext),
            tm_load_header("tp-1", current.hash()),
            tm_send("tp-1", "manager", ManagerMessage::RemovePeer(peer)),
            tm_state("tp-1", &expected),
        ],
    );
    logs.assert_and_remove(Level::ERROR, &["chain_sync.roll_backward.failed", "Unknown point"])
        .assert_and_remove(Level::INFO, &["roll backward"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}
