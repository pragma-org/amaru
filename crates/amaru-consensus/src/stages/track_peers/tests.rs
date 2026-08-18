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

use amaru_kernel::{BlockHeight, Epoch, EraHistory, EraName, HeaderHash, IsHeader, Peer, Point, num::CheckedSub};
use amaru_ouroboros::ConnectionId;
use amaru_ouroboros_traits::{Nonces, has_stake_distribution::GetPoolError};
use amaru_protocols::chainsync::{
    self, ChainSyncInitiatorMsg, HeaderContent, InitiatorMessage, InitiatorMessage::RequestNext,
};
use amaru_pure_stage::{
    Instant, assert_trace_contains, assert_trace_does_not_contain, assert_trace_match,
    simulation::running::OverrideResult, tm_send,
};
use tracing::Level;

use crate::{
    effects::{ValidateHeaderEffect, VolatileTipEffect},
    errors::ConsensusError,
    stages::{
        peer_selection::PeerSelectionMsg,
        test_utils::{start_in_era, te_clock_read, te_input, te_send, te_state, tm_state},
        track_peers::{
            TrackPeers, TrackPeersMsg,
            test_setup::{
                HEIGHT_RECHECK_INTERVAL, SIM_INITIAL_CLOCK_SECS, build_store, build_store_with_nonces,
                height_recheck_schedule_id, make_block_header, new_tip, schedule_id_at, setup, setup_base,
                setup_with_ledger_tip_until_sleeping, slot_start_to_header_micros, te_clear_peer_availability,
                te_clock, te_clock_suspend, te_get_nonces, te_header_rejected, te_load_header, te_load_point,
                te_record_header_announcement, te_record_rollback, te_schedule, te_store_validated_header,
                te_validate_header, test_prep, test_prep_with_max_peer_lead, tm_volatile_tip,
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
    assert_trace_contains(
        &running,
        &[te_state("tp-1", &state).into(), te_input("tp-1", &msg).into(), te_state("tp-1", &expected).into()],
    );
    logs.assert_and_remove(Level::INFO, &["chainsync.initialized"]).assert_no_remaining_at([
        Level::DEBUG,
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
    state.insert_peer(peer.clone(), prep.conn_id, Point::Origin, Point::Origin);
    state.push_deferred_for_tests(peer.clone(), prep.conn_id, prep.handler.clone(), header.clone(), header.point());
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::Initialize,
    });

    let mut expected = prep.state.clone();
    expected.record_connecting(peer, prep.conn_id);

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace_contains(
        &running,
        &[te_state("tp-1", &state).into(), te_input("tp-1", &msg).into(), te_state("tp-1", &expected).into()],
    );
    logs.assert_and_remove(Level::WARN, &["chainsync.reinitialized"])
        .assert_and_remove(Level::INFO, &["chainsync.initialized"])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_terminated_purges_upstream_and_deferred() {
    let prep = test_prep_with_max_peer_lead(0);
    let peer = Peer::new("peer1");
    let parent = &prep.headers[0];
    let header = &prep.headers[1];
    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), prep.conn_id, parent.point(), parent.point());
    state.push_deferred_for_tests(peer.clone(), prep.conn_id, prep.handler.clone(), header.clone(), header.point());

    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::Terminated,
    });

    let expected = prep.state.clone(); // empty upstream + deferred

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace_contains(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_clear_peer_availability("tp-1", peer).into(),
            te_state("tp-1", &expected).into(),
        ],
    );
    logs.assert_and_remove(Level::INFO, &["chainsync.terminated"]).assert_no_remaining_at([
        Level::DEBUG,
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
    state.insert_peer(peer.clone(), conn_a, Point::Origin, Point::Origin);
    state.insert_peer(peer.clone(), conn_b, prep.headers[0].point(), prep.headers[0].point());

    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: conn_a,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::Terminated,
    });

    let mut expected = prep.state.clone();
    expected.insert_peer(peer.clone(), conn_b, prep.headers[0].point(), prep.headers[0].point());

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace_contains(
        &running,
        &[te_state("tp-1", &state).into(), te_input("tp-1", &msg).into(), te_state("tp-1", &expected).into()],
    );
    // Other connection still tracked for this peer ⇒ no clear_peer_availability.
    assert_trace_does_not_contain(&running, &[te_clear_peer_availability("tp-1", peer).into()]);
    logs.assert_and_remove(Level::INFO, &["chainsync.terminated"]).assert_no_remaining_at([
        Level::DEBUG,
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

#[test]
fn test_intersect_found_missing_header_sends_done() {
    let prep = test_prep();
    let state = prep.state.clone();
    let current = Point::Specific(1u64.into(), HeaderHash::from([1u8; 32]), BlockHeight::from(1));
    let tip = current;
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: Peer::new("peer1"),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::IntersectFound(current, tip),
    });

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace_contains(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_load_point("tp-1", current.hash()).into(),
            te_send("tp-1", &prep.handler, chainsync::InitiatorMessage::Done).into(),
            te_state("tp-1", &state).into(),
        ],
    );
    logs.assert_and_remove(Level::WARN, &["chainsync.unknown_intersection_point"]).assert_no_remaining_at([
        Level::DEBUG,
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
    let tip = prep.headers[1].point();
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: Peer::new("peer1"),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::IntersectFound(current, tip),
    });

    let mut expected = state.clone();
    expected.insert_peer(Peer::new("peer1"), prep.conn_id, header.point(), tip);

    let (running, _guards, mut logs) =
        setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(slice::from_ref(header)));
    assert_trace_contains(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_load_point("tp-1", current.hash()).into(),
            te_state("tp-1", &expected).into(),
        ],
    );
    logs.assert_and_remove(Level::INFO, &["chainsync.intersect_found"]).assert_no_remaining_at([
        Level::DEBUG,
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
        msg: chainsync::InitiatorResult::IntersectNotFound(Point::Origin),
    });

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace_contains(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_send("tp-1", &prep.handler, chainsync::InitiatorMessage::Done).into(),
            te_state("tp-1", &state).into(),
        ],
    );
    logs.assert_and_remove(Level::INFO, &["chainsync.intersect_not_found"]).assert_no_remaining_at([
        Level::DEBUG,
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
    state.insert_peer(peer.clone(), prep.conn_id, Point::Origin, Point::Origin);
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::IntersectNotFound(Point::Origin),
    });

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace_contains(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_send("tp-1", &prep.handler, chainsync::InitiatorMessage::Done).into(),
            te_state("tp-1", &expected).into(),
        ],
    );
    logs.assert_and_remove(Level::INFO, &["chainsync.intersect_not_found"]).assert_no_remaining_at([
        Level::DEBUG,
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
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(header, EraName::Conway), child.point()),
    });

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace_match(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_clock_suspend("tp-1").into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            te_header_rejected("invalid header").into(),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer)).into(),
            te_state("tp-1", &state).into(),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="peer1""#])
        .assert_and_remove(Level::DEBUG, &["perf.header.lifecycle", r#"outcome="invalid_header""#])
        .assert_and_remove(Level::ERROR, &["perf.header.lifecycle", "Unknown peer"])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
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
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(header, EraName::Conway), header.point()),
    });

    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), prep.conn_id, parent.point(), parent.point());

    let mut expected = prep.state.clone();
    expected.insert_peer(peer.clone(), prep.conn_id, header.point(), header.point());

    let received_at = Instant::at_offset(Duration::from_secs(SIM_INITIAL_CLOCK_SECS), start_in_era().relative_time);
    let (running, _guards, mut logs) =
        setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store_with_nonces(slice::from_ref(header)));
    assert_trace_match(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_clock_suspend("tp-1").into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            te_get_nonces("tp-1", header.hash()).into(),
            te_record_header_announcement(
                "tp-1",
                peer.clone(),
                header.point(),
                header.parent_hash(),
                received_at,
                slot_start_to_header_micros(&header.point(), received_at),
            )
            .into(),
            te_header_rejected("duplicate header").into(),
            te_state("tp-1", &expected).into(),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["chainsync.roll_forward_done", r#"outcome="already_stored""#])
        .assert_and_remove(Level::DEBUG, &["perf.header.lifecycle", r#"outcome="duplicate_header""#])
        .assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="peer1""#])
        .assert_and_remove(Level::DEBUG, &["perf.header.lifecycle", r#"outcome="duplicate_header""#])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

/// A header may already sit in the chain store without nonces (legacy import / incomplete
/// migration). When re-received from a peer, its nonces must still be computed so descendant
/// headers can be validated. Nonce absence means the header was never fully validated, so it is
/// treated like a new header: stored with its nonces and propagated downstream. `select_chain`
/// accepts the resulting tip even if concurrent recovery already validated the block body.
#[test]
fn test_roll_forward_stored_header_missing_nonces_revalidates() {
    let prep = test_prep();
    let peer = Peer::new("peer1");
    let parent = &prep.headers[0];
    let header = &prep.headers[1];
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(header, EraName::Conway), header.point()),
    });

    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), prep.conn_id, parent.point(), parent.point());

    let mut expected = prep.state.clone();
    expected.insert_peer(peer.clone(), prep.conn_id, header.point(), header.point());

    // Header present but nonces absent, as after a bootstrap import.
    let received_at = Instant::at_offset(Duration::from_secs(SIM_INITIAL_CLOCK_SECS), start_in_era().relative_time);
    let (running, _guards, mut logs) =
        setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(slice::from_ref(header)));
    assert_trace_contains(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_clock_suspend("tp-1").into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            te_get_nonces("tp-1", header.hash()).into(),
            te_validate_header("tp-1", header.clone()).into(),
            te_store_validated_header("tp-1", header.clone()).into(),
            te_record_header_announcement(
                "tp-1",
                peer.clone(),
                header.point(),
                header.parent_hash(),
                received_at,
                slot_start_to_header_micros(&header.point(), received_at),
            )
            .into(),
            te_send("tp-1", "downstream", new_tip(header.point(), parent.point())).into(),
            te_state("tp-1", &expected).into(),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["chainsync.roll_forward_done", r#"outcome="stored""#])
        .assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="peer1""#])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
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
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(header, EraName::Conway), header.point()),
    });

    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), prep.conn_id, parent.point(), parent.point());

    let mut expected = prep.state.clone();
    expected.insert_peer(peer.clone(), prep.conn_id, header.point(), header.point());

    let received_at = Instant::at_offset(Duration::from_secs(SIM_INITIAL_CLOCK_SECS), start_in_era().relative_time);
    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace_contains(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_clock_suspend("tp-1").into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            te_get_nonces("tp-1", header.hash()).into(),
            te_validate_header("tp-1", header.clone()).into(),
            te_store_validated_header("tp-1", header.clone()).into(),
            te_record_header_announcement(
                "tp-1",
                peer.clone(),
                header.point(),
                header.parent_hash(),
                received_at,
                slot_start_to_header_micros(&header.point(), received_at),
            )
            .into(),
            te_send("tp-1", "downstream", new_tip(header.point(), parent.point())).into(),
            te_state("tp-1", &expected).into(),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["chainsync.roll_forward_done", r#"outcome="stored""#])
        .assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="peer1""#])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
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
        msg: chainsync::InitiatorResult::RollForward(
            HeaderContent::with_bytes(vec![], EraName::Babbage),
            parent.point(),
        ),
    });

    let expected = prep.state.clone();
    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), prep.conn_id, parent.point(), parent.point());

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace_contains(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_header_rejected("undecodable header").into(),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer)).into(),
            te_state("tp-1", &expected).into(),
        ],
    );
    logs.assert_and_remove(Level::ERROR, &["perf.header.lifecycle", "Invalid header variant"])
        .assert_and_remove(Level::DEBUG, &["perf.header.lifecycle", r#"outcome="undecodable_header""#])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
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
            parent.point(),
        ),
    });

    let expected = prep.state.clone();
    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), prep.conn_id, parent.point(), parent.point());

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace_contains(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_header_rejected("undecodable header").into(),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer)).into(),
            te_state("tp-1", &expected).into(),
        ],
    );
    logs.assert_and_remove(Level::ERROR, &["perf.header.lifecycle", "Failed to decode header"])
        .assert_and_remove(Level::DEBUG, &["perf.header.lifecycle", r#"outcome="undecodable_header""#])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
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
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&header, EraName::Conway), header.point()),
    });

    let expected = prep.state.clone();
    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), prep.conn_id, parent.point(), parent.point());

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace_match(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_clock_suspend("tp-1").into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            te_header_rejected("invalid header").into(),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer)).into(),
            te_state("tp-1", &expected).into(),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="peer1""#])
        .assert_and_remove(Level::DEBUG, &["perf.header.lifecycle", r#"outcome="invalid_header""#])
        .assert_and_remove(Level::ERROR, &["perf.header.lifecycle", "Invalid header parent"])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
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
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&header, EraName::Conway), header.point()),
    });

    let expected = prep.state.clone();
    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), prep.conn_id, parent.point(), parent.point());

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace_match(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_clock_suspend("tp-1").into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            te_header_rejected("invalid header").into(),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer)).into(),
            te_state("tp-1", &expected).into(),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="peer1""#])
        .assert_and_remove(Level::DEBUG, &["perf.header.lifecycle", r#"outcome="invalid_header""#])
        .assert_and_remove(Level::ERROR, &["perf.header.lifecycle", "Invalid header height"])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
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
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&header, EraName::Conway), parent.point()),
    });

    let expected = prep.state.clone();
    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), prep.conn_id, parent.point(), parent.point());

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace_match(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_clock_suspend("tp-1").into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            te_header_rejected("invalid header").into(),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer)).into(),
            te_state("tp-1", &expected).into(),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="peer1""#])
        .assert_and_remove(Level::DEBUG, &["perf.header.lifecycle", r#"outcome="invalid_header""#])
        .assert_and_remove(Level::ERROR, &["perf.header.lifecycle", "Invalid header point"])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
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
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(header, EraName::Conway), header.point()),
    });

    let expected = prep.state.clone();
    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), prep.conn_id, parent.point(), header.point());

    // Use empty store so evolve_nonce fails (unknown parent), exercising the real validate_header fn failure path.
    let (running, _guards, mut logs) =
        setup_base(&prep.rt_handle(), state.clone(), [msg.clone()], build_store(&[]), |running| {
            let header = header.hash();
            let parent = parent.hash();
            running.override_external_effect::<ValidateHeaderEffect>(usize::MAX, move |_| {
                OverrideResult::handled(Err(ValidateHeaderError::Nonces(NoncesError::UnknownParent { header, parent })))
            });
        });

    logs.assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="peer1""#])
        .assert_and_remove(Level::DEBUG, &["perf.header.lifecycle", r#"outcome="invalid_header""#])
        .assert_and_remove(Level::ERROR, &["perf.header.lifecycle"])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
    assert_trace_match(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_clock_suspend("tp-1").into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            te_get_nonces("tp-1", header.hash()).into(),
            te_validate_header("tp-1", header.clone()).into(),
            te_header_rejected("invalid header").into(),
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
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&header, EraName::Conway), header.point()),
    });

    let expected = prep.state.clone();
    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), prep.conn_id, parent.point(), header.point());

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));

    logs.assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="peer1""#])
        .assert_and_remove(Level::DEBUG, &["perf.header.lifecycle", r#"outcome="invalid_header""#])
        .assert_and_remove(Level::ERROR, &["perf.header.lifecycle"])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
    assert_trace_match(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_clock_suspend("tp-1").into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            te_header_rejected("invalid header").into(),
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
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&header, EraName::Conway), header.point()),
    });

    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), prep.conn_id, parent.point(), header.point());

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));

    logs.assert_and_remove(Level::DEBUG, &["chainsync.header_deferred", r#"reason="clock_skew""#])
        .assert_and_remove(Level::DEBUG, &["chainsync.roll_forward_done", r#"outcome="stored""#])
        .assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="peer1""#])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
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
            te_store_validated_header("tp-1", header.clone()).into(),
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
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(header, EraName::Conway), header.point()),
    });

    let expected = prep.state.clone();
    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), prep.conn_id, parent.point(), header.point());

    // More than one epoch beyond known max_epoch (start-2) → adversarial, not defer.
    let far_epoch = prep.start_times.epoch;
    let slot = header.slot();
    // Override to simulate far-ahead stake dist not available (distance >1 -> reject)
    let (running, _guards, mut logs) =
        setup_base(&prep.rt_handle(), state.clone(), [msg.clone()], build_store(&[]), |running| {
            running.override_external_effect::<ValidateHeaderEffect>(usize::MAX, move |_| {
                OverrideResult::handled(Err(ValidateHeaderError::Consensus(ConsensusError::GetPoolError(
                    GetPoolError::StakeDistributionNotAvailable(slot, Some(far_epoch)),
                ))))
            });
        });

    logs.assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="peer1""#])
        .assert_and_remove(Level::DEBUG, &["perf.header.lifecycle", r#"outcome="invalid_header""#])
        .assert_and_remove(Level::ERROR, &["perf.header.lifecycle"])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
    assert_trace_match(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_clock_suspend("tp-1").into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            te_get_nonces("tp-1", header.hash()).into(),
            te_validate_header("tp-1", header.clone()).into(),
            te_header_rejected("invalid header").into(),
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
    let tip = current;
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollBackward(current, tip),
    });

    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), prep.conn_id, Point::Origin, Point::Origin);

    let mut expected = prep.state.clone();
    expected.insert_peer(peer.clone(), prep.conn_id, header.point(), tip);

    let now = Instant::at_offset(Duration::from_secs(SIM_INITIAL_CLOCK_SECS), start_in_era().relative_time);
    let (running, _guards, mut logs) =
        setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(slice::from_ref(header)));
    assert_trace_contains(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            te_load_point("tp-1", current.hash()).into(),
            te_load_header("tp-1", current.hash()).into(),
            te_clock_read("tp-1").into(),
            te_record_rollback("tp-1", peer, header.point(), header.parent_hash(), now).into(),
            te_state("tp-1", &expected).into(),
        ],
    );
    logs.assert_and_remove(Level::INFO, &["chainsync.roll_backward"]).assert_no_remaining_at([
        Level::DEBUG,
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
        msg: chainsync::InitiatorResult::RollBackward(current, Point::Origin),
    });

    let state = prep.state.clone();

    let (running, _guards, mut logs) =
        setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(slice::from_ref(header)));
    assert_trace_contains(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            te_load_point("tp-1", current.hash()).into(),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer)).into(),
            te_state("tp-1", &state).into(),
        ],
    );
    logs.assert_and_remove(Level::ERROR, &["chainsync.roll_backward_failed", "Unknown peer"])
        .assert_and_remove(Level::INFO, &["chainsync.roll_backward"])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_roll_backward_unknown_point_removes_peer() {
    let prep = test_prep();
    let peer = Peer::new("peer1");
    let current = Point::Specific(1u64.into(), HeaderHash::from([1u8; 32]), BlockHeight::from(1));
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollBackward(current, Point::Origin),
    });

    let expected = prep.state.clone();
    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), prep.conn_id, Point::Origin, Point::Origin);

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace_contains(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            te_load_point("tp-1", current.hash()).into(),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer)).into(),
            te_state("tp-1", &expected).into(),
        ],
    );
    logs.assert_and_remove(Level::ERROR, &["chainsync.roll_backward_failed", "Unknown point"])
        .assert_and_remove(Level::INFO, &["chainsync.roll_backward"])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

/// Tests that a RollForward whose header height requires a ledger height beyond what is currently
/// applied defers RequestNext and arms a single coalesced height-recheck schedule.
#[test]
fn test_roll_forward_defers_request_next() {
    // Use max_peer_lead = 0 so any header taller than the known ledger height triggers defer.
    let prep = test_prep_with_max_peer_lead(0);
    let peer = Peer::new("peer1");
    let header = prep.headers[0].clone();
    let tip = header.point();

    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), prep.conn_id, Point::Origin, tip);

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
        setup_with_ledger_tip_until_sleeping(&prep.rt_handle(), state.clone(), [msg.clone()], store, Point::Origin);

    logs.assert_and_remove(
        Level::DEBUG,
        &["chainsync.header_deferred", r#"reason="ledger_height""#, "header_height=1", "ledger_height=0", "limit=1"],
    )
    .assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="peer1""#])
    .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);

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
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&h1, EraName::Conway), h1.point()),
    });
    let msg2 = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&h2, EraName::Conway), h2.point()),
    });

    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), prep.conn_id, parent.point(), h2.point());

    let sid = height_recheck_schedule_id();

    // Forced ledger tip = origin so height defers apply; second header is FollowUp while peer deferred.
    // Stop at first sleep so the height-poll loop does not run forever under a frozen tip.
    let (running, _guards, mut logs) = setup_with_ledger_tip_until_sleeping(
        &prep.rt_handle(),
        state.clone(),
        [msg1.clone(), msg2.clone()],
        build_store(&[]),
        Point::Origin,
    );

    logs.assert_and_remove(Level::DEBUG, &["chainsync.header_deferred", r#"reason="ledger_height""#, "limit=2"])
        .assert_and_remove(Level::DEBUG, &["chainsync.header_deferred", r#"reason="follow_up""#])
        .assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="peer1""#])
        .assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="peer1""#])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
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
            te_clock_suspend("tp-1").into(),
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
    let tip = header.point();

    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), prep.conn_id, Point::Origin, tip);

    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&header, EraName::Conway), tip),
    });

    let sid = height_recheck_schedule_id();
    let recheck_at = schedule_id_at(HEIGHT_RECHECK_INTERVAL).time();
    let advanced_tip = header.point();

    let (running, _guards, mut logs) =
        setup_base(&prep.rt_handle(), state.clone(), [msg.clone()], build_store(&[]), |running| {
            let mut n = 0u8;
            running.override_external_effect::<VolatileTipEffect>(usize::MAX, move |_| {
                n += 1;
                // First call (defer decision) still at origin; recheck sees advanced height.
                if n == 1 { OverrideResult::handled(Point::Origin) } else { OverrideResult::handled(advanced_tip) }
            });
            running.override_external_effect::<ValidateHeaderEffect>(usize::MAX, |_| {
                OverrideResult::handled(Ok(Nonces::for_tests()))
            });
        });

    logs.assert_and_remove(Level::DEBUG, &["chainsync.header_deferred", r#"reason="ledger_height""#])
        .assert_and_remove(Level::DEBUG, &["chainsync.roll_forward_done", r#"outcome="stored""#])
        .assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="peer1""#])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);

    assert_trace_contains(
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
            te_get_nonces("tp-1", header.hash()).into(),
            te_validate_header("tp-1", header.clone()).into(),
            te_store_validated_header("tp-1", header.clone()).into(),
            te_send("tp-1", "downstream", new_tip(header.point(), Point::Origin)).into(),
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
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&h1, EraName::Conway), h1.point()),
    });
    let msg2 = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&h2, EraName::Conway), h2.point()),
    });

    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), prep.conn_id, parent.point(), h2.point());

    let (running, _guards, mut logs) =
        setup_base(&prep.rt_handle(), state.clone(), [msg1.clone(), msg2.clone()], build_store(&[]), |running| {
            running.override_external_effect::<ValidateHeaderEffect>(usize::MAX, |_| {
                OverrideResult::handled(Ok(Nonces::for_tests()))
            });
        });

    let (h1_hash, h2_hash) = (h1.hash().to_string(), h2.hash().to_string());
    // h2 is one slot later than h1, so it is still in the near future when h1 becomes valid and
    // gets clock-skew deferred a second time, on its own this time rather than as a follow-up.
    logs.assert_and_remove(Level::DEBUG, &["chainsync.header_deferred", r#"reason="clock_skew""#, &h1_hash])
        .assert_and_remove(Level::DEBUG, &["chainsync.header_deferred", r#"reason="follow_up""#, &h2_hash])
        .assert_and_remove(Level::DEBUG, &["chainsync.roll_forward_done", r#"outcome="stored""#, &h1_hash])
        .assert_and_remove(Level::DEBUG, &["chainsync.header_deferred", r#"reason="clock_skew""#, &h2_hash])
        .assert_and_remove(Level::DEBUG, &["chainsync.roll_forward_done", r#"outcome="stored""#, &h2_hash])
        .assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="peer1""#])
        .assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="peer1""#])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
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
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&h1, EraName::Conway), h1.point()),
    });
    let msg2 = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&h2, EraName::Conway), h2.point()),
    });
    // Advance max_epoch far enough that the previously missing target epoch is covered.
    let wake = TrackPeersMsg::StakeDistUpdated(prep.start_times.epoch);

    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), prep.conn_id, parent.point(), h2.point());

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
                    OverrideResult::handled(Err(ValidateHeaderError::Consensus(ConsensusError::GetPoolError(
                        GetPoolError::StakeDistributionNotAvailable(slot1, Some(target_epoch)),
                    ))))
                } else {
                    OverrideResult::handled(Ok(Nonces::for_tests()))
                }
            });
        },
    );

    let (h1_hash, h2_hash) = (h1.hash().to_string(), h2.hash().to_string());
    logs.assert_and_remove(Level::DEBUG, &["chainsync.header_deferred", r#"reason="stake_distribution""#, &h1_hash])
        .assert_and_remove(Level::DEBUG, &["chainsync.header_deferred", r#"reason="follow_up""#, &h2_hash])
        .assert_and_remove(Level::DEBUG, &["chainsync.roll_forward_done", r#"outcome="stored""#, &h1_hash])
        .assert_and_remove(Level::DEBUG, &["chainsync.roll_forward_done", r#"outcome="stored""#, &h2_hash])
        .assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="peer1""#])
        .assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="peer1""#])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
    // h1 stake-deferred after RN; h2 is FollowUp (peer already deferred); wake reprocesses both in order.
    assert_trace_contains(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg1).into(),
            te_clock_suspend("tp-1").into(),
            te_send("tp-1", &prep.handler, RequestNext).into(),
            te_get_nonces("tp-1", h1.hash()).into(),
            te_validate_header("tp-1", h1.clone()).into(),
            tm_state::<TrackPeers>("tp-1", |s| s.deferred.len() == 1, "first stake deferred"),
            te_input("tp-1", &msg2).into(),
            te_clock_suspend("tp-1").into(),
            tm_state::<TrackPeers>("tp-1", |s| s.deferred.len() == 2, "follow-up queued"),
            te_input("tp-1", &wake).into(),
            tm_volatile_tip("tp-1"),
            te_clock_suspend("tp-1").into(),
            te_get_nonces("tp-1", h1.hash()).into(),
            te_validate_header("tp-1", h1.clone()).into(),
            te_store_validated_header("tp-1", h1.clone()).into(),
            te_send("tp-1", "downstream", new_tip(h1.point(), parent.point())).into(),
            te_get_nonces("tp-1", h2.hash()).into(),
            te_validate_header("tp-1", h2.clone()).into(),
            te_store_validated_header("tp-1", h2.clone()).into(),
            te_send("tp-1", "downstream", new_tip(h2.point(), h1.point())).into(),
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

/// Two headers deferred for the same connection; on recheck the first fails validation and
/// purges the connection, which also drops the second entry from the deferred list.
/// Regression: the recheck loop used to index past the shrunk list and panic.
#[test]
fn test_recheck_deferred_survives_purge_shrinking_the_list() {
    let prep = test_prep();
    let peer = Peer::new("peer1");
    let parent = &prep.headers[0];
    let wrong_parent = HeaderHash::from([9u8; 32]);
    let h1 = make_block_header(2, 2, Some(wrong_parent));
    let h2 = make_block_header(3, 3, Some(h1.hash()));

    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), prep.conn_id, parent.point(), parent.point());
    state.push_deferred_for_tests(peer.clone(), prep.conn_id, prep.handler.clone(), h1.clone(), h1.point());
    state.push_deferred_for_tests(peer.clone(), prep.conn_id, prep.handler.clone(), h2.clone(), h2.point());

    let msg = TrackPeersMsg::RecheckLedgerHeight;

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace_match(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            tm_volatile_tip("tp-1"),
            te_clock_suspend("tp-1").into(),
            te_header_rejected("invalid header").into(),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer)).into(),
            tm_state::<TrackPeers>(
                "tp-1",
                |s| s.deferred.is_empty() && s.upstream.is_empty(),
                "connection purged with all its deferred entries",
            ),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["perf.header.lifecycle", r#"outcome="invalid_header""#])
        .assert_and_remove(Level::ERROR, &["perf.header.lifecycle", "Invalid header parent"])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

/// A deferred header that is still deferred on recheck must keep blocking its follow-ups.
/// The peer tip has not advanced, so validating a follow-up would wrongly flag the peer as adversarial.
#[test]
fn test_redeferred_header_keeps_blocking_follow_ups() {
    let prep = test_prep();
    let peer = Peer::new("peer1");
    let parent = &prep.headers[0];
    let h1 = prep.headers[1].clone();
    let h2 = make_block_header(3, h1.slot().as_u64() + 1, Some(h1.hash()));

    let msg1 = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&h1, EraName::Conway), h1.point()),
    });
    let msg2 = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer: peer.clone(),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&h2, EraName::Conway), h2.point()),
    });
    // One epoch ahead of known max_epoch (start-2). We defer the header.
    let first_target = prep.start_times.epoch.checked_sub(Epoch::ONE).unwrap();
    // The stake distribution is updated but the header stays deferred.
    let stake_distribution_update = TrackPeersMsg::StakeDistUpdated(first_target);
    let second_target = prep.start_times.epoch;

    let mut state = prep.state.clone();
    state.insert_peer(peer.clone(), prep.conn_id, parent.point(), h2.point());

    let slot1 = h1.slot();
    let (running, _guards, mut logs) = setup_base(
        &prep.rt_handle(),
        state.clone(),
        [msg1.clone(), msg2.clone(), stake_distribution_update.clone()],
        build_store(&[]),
        |running| {
            let mut n = 0u8;
            running.override_external_effect::<ValidateHeaderEffect>(usize::MAX, move |_| {
                n += 1;
                let target = if n == 1 { first_target } else { second_target };
                OverrideResult::handled(Err(ValidateHeaderError::Consensus(ConsensusError::GetPoolError(
                    GetPoolError::StakeDistributionNotAvailable(slot1, Some(target)),
                ))))
            });
        },
    );

    let (h1_hash, h2_hash) = (h1.hash().to_string(), h2.hash().to_string());
    // h1 is deferred for the same reason twice: once while its roll-forward is handled, then again
    // when the recheck re-validates it and the stake distribution it needs is still missing. Only
    // the first happens inside the roll-forward span, which is what tells the two apart.
    logs.assert_and_remove(
        Level::DEBUG,
        &["roll_forward.process", "chainsync.header_deferred", r#"reason="stake_distribution""#, &h1_hash],
    )
    .assert_and_remove(Level::DEBUG, &["chainsync.header_deferred", r#"reason="follow_up""#, &h2_hash])
    .assert_and_remove(Level::DEBUG, &["chainsync.header_deferred", r#"reason="stake_distribution""#, &h1_hash])
    .assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="peer1""#])
    .assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="peer1""#])
    .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
    assert_trace_contains(
        &running,
        &[
            te_input("tp-1", &msg1).into(),
            tm_state::<TrackPeers>("tp-1", |s| s.deferred.len() == 1, "first header deferred"),
            te_input("tp-1", &msg2).into(),
            tm_state::<TrackPeers>("tp-1", |s| s.deferred.len() == 2, "its follow-up is queued"),
            te_input("tp-1", &stake_distribution_update).into(),
            tm_state::<TrackPeers>(
                "tp-1",
                |s| s.deferred.len() == 2 && !s.upstream.is_empty(),
                "both headers are still deferred after recheck. The connection is active",
            ),
        ],
    );
    assert_trace_does_not_contain(&running, &[tm_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer))]);
}
