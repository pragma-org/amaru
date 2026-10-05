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

use amaru_kernel::{
    BlockHeight, Epoch, EraHistory, EraName, Header, HeaderHash, IsHeader, Peer, Point, num::CheckedSub,
};
use amaru_observability::tracing::Level;
use amaru_ouroboros::ConnectionId;
use amaru_ouroboros_traits::{Nonces, has_stake_distribution::GetPoolError};
use amaru_protocols::chainsync::{
    self, ChainSyncInitiatorMsg, HeaderContent, InitiatorMessage, InitiatorMessage::RequestNext, PIPELINE_DEPTH,
};
use amaru_pure_stage::{
    DEFAULT_MAILBOX_SIZE, Effect, Instant, StageRef, StageResponse, TrySend, assert_trace_contains,
    assert_trace_does_not_contain, assert_trace_match, assert_trace_match_filter,
    simulation::{Blocked, Run, running::OverrideResult},
    tm_send, tm_try_send,
    trace_buffer::TraceEntry,
};

fn tm_any_request_next() -> amaru_pure_stage::TraceMatch<'static> {
    amaru_pure_stage::TraceMatch::Property(
        Box::new(|src| {
            let msg = match src.suspend() {
                Some(Effect::Send { msg, .. } | Effect::TrySend { msg, .. }) => msg,
                _ => return false,
            };
            msg.cast_ref::<InitiatorMessage>().is_ok_and(|message| matches!(message, InitiatorMessage::RequestNext))
        }),
        "RequestNext via send or try_send".to_string(),
    )
}

use crate::{
    consensus_mode::{ChainLagSample, tip_lateness},
    effects::{ValidateHeaderEffect, VolatileTipEffect},
    errors::ConsensusError,
    stages::{
        peer_selection::PeerSelectionMsg,
        test_utils::{start_in_era, te_clock_read, te_input, te_send, te_state, tm_state},
        track_peers::{
            TrackPeers, TrackPeersMsg,
            test_setup::{
                HEIGHT_RECHECK_INTERVAL, HandlerHold, SIM_INITIAL_CLOCK_SECS, build_store, build_store_with_nonces,
                height_recheck_schedule_id, make_block_header, new_tip, open_fanout, open_fanout_quick_then_hour,
                schedule_id_at, setup, setup_base, setup_with_ledger_tip_until_sleeping, slot_start_to_header_micros,
                te_clear_peer_availability, te_clock, te_clock_suspend, te_get_best_chain_tip, te_get_nonces,
                te_header_rejected, te_load_header, te_load_point, te_record_header_announcement, te_record_rollback,
                te_schedule, te_store_validated_header, te_sync_adoption_is_fast, te_validate_header, test_prep,
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
    let peer = Peer::for_test(3001);
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
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
    let peer = Peer::for_test(3001);
    let header = &prep.headers[1];
    let mut state = prep.state.clone();
    state.insert_peer(peer, prep.conn_id, Point::Origin, Point::Origin);
    state.push_deferred_for_tests(peer, prep.conn_id, prep.handler.clone(), header.clone(), header.point());
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
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
    let peer = Peer::for_test(3001);
    let parent = &prep.headers[0];
    let header = &prep.headers[1];
    let mut state = prep.state.clone();
    state.insert_peer(peer, prep.conn_id, parent.point(), parent.point());
    state.push_deferred_for_tests(peer, prep.conn_id, prep.handler.clone(), header.clone(), header.point());

    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
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
    let peer = Peer::for_test(3001);
    let mut other_id = ConnectionId::initial();
    let conn_a = other_id.get_and_increment();
    let conn_b = other_id.get_and_increment();

    let mut state = prep.state.clone();
    state.insert_peer(peer, conn_a, Point::Origin, Point::Origin);
    state.insert_peer(peer, conn_b, prep.headers[0].point(), prep.headers[0].point());

    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
        conn_id: conn_a,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::Terminated,
    });

    let mut expected = prep.state.clone();
    expected.insert_peer(peer, conn_b, prep.headers[0].point(), prep.headers[0].point());

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
        peer: Peer::for_test(3001),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::IntersectFound(current, tip),
    });

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    let admission = running.trace_buffer().lock().hydrate_without_timestamps();
    assert_trace_contains(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_load_point("tp-1", current.hash()).into(),
            tm_try_send("tp-1", "", chainsync::InitiatorMessage::Done),
            te_state("tp-1", &state).into(),
        ],
    );
    assert_try_send_resumes(&admission, &[TrySend::Queued]);
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
        peer: Peer::for_test(3001),
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::IntersectFound(current, tip),
    });

    let mut expected = state.clone();
    expected.insert_peer(Peer::for_test(3001), prep.conn_id, header.point(), tip);

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
fn test_reconnect_intersect_then_roll_forward() {
    let prep = test_prep();
    let peer = Peer::for_test(3001);
    let mut ids = ConnectionId::initial();
    let conn0 = ids.get_and_increment();
    let conn1 = ids.get_and_increment();
    let intersect_header = &prep.headers[0];
    let next_header = &prep.headers[1];
    let stale = &prep.headers[2];
    let intersect = intersect_header.point();

    let mut state = prep.state.clone();
    state.insert_peer(peer, conn0, stale.point(), stale.point());

    let terminated = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
        conn_id: conn0,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::Terminated,
    });
    let initialize = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
        conn_id: conn1,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::Initialize,
    });
    let intersect_found = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
        conn_id: conn1,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::IntersectFound(intersect, stale.point()),
    });
    let roll_forward = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
        conn_id: conn1,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(next_header, EraName::Conway), stale.point()),
    });

    let mut expected = prep.state.clone();
    expected.insert_peer(peer, conn1, next_header.point(), stale.point());

    let (running, _guards, mut logs) = setup_base(
        &prep.rt_handle(),
        state,
        [terminated.clone(), initialize.clone(), intersect_found.clone(), roll_forward.clone()],
        build_store(slice::from_ref(intersect_header)),
        |running| {
            running.override_external_effect::<ValidateHeaderEffect>(usize::MAX, |_| {
                OverrideResult::handled(Ok(Nonces::for_tests()))
            });
        },
    );

    let admission = running.trace_buffer().lock().hydrate_without_timestamps();
    assert_trace_contains(
        &running,
        &[
            te_input("tp-1", &terminated).into(),
            te_clear_peer_availability("tp-1", peer).into(),
            te_input("tp-1", &initialize).into(),
            te_input("tp-1", &intersect_found).into(),
            te_load_point("tp-1", intersect.hash()).into(),
            te_input("tp-1", &roll_forward).into(),
            tm_try_send("tp-1", "", RequestNext),
            te_send("tp-1", "downstream", new_tip(next_header.point(), intersect)).into(),
            te_state("tp-1", &expected).into(),
        ],
    );
    assert_try_send_resumes(&admission, &[TrySend::Queued]);
    assert_trace_does_not_contain(&running, &[tm_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer))]);
    logs.assert_and_remove(Level::INFO, &["chainsync.terminated"])
        .assert_and_remove(Level::INFO, &["chainsync.initialized"])
        .assert_and_remove(Level::INFO, &["chainsync.intersect_found"])
        .assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="127.0.0.1:3001""#])
        .assert_and_remove(Level::DEBUG, &["chainsync.roll_forward_done", r#"outcome="stored""#])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_intersect_not_found_untracked_notifies_uninteresting() {
    let prep = test_prep();
    let state = prep.state.clone();
    let peer = Peer::for_test(3001);
    let conn_id = prep.conn_id;
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
        conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::IntersectNotFound(Point::Origin),
    });

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    assert_trace_contains(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::Uninteresting { peer, conn_id, after_rollback: false })
                .into(),
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
    let peer = Peer::for_test(3001);
    let expected = prep.state.clone();
    let mut state = prep.state.clone();
    state.insert_peer(peer, prep.conn_id, Point::Origin, Point::Origin);
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
            te_send(
                "tp-1",
                "peer_selection",
                PeerSelectionMsg::Uninteresting { peer, conn_id: prep.conn_id, after_rollback: false },
            )
            .into(),
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
    let peer = Peer::for_test(3001);
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(header, EraName::Conway), child.point()),
    });

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    let admission = running.trace_buffer().lock().hydrate_without_timestamps();
    assert_trace_match(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_clock_suspend("tp-1").into(),
            tm_try_send("tp-1", "", RequestNext),
            te_header_rejected("invalid header").into(),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer)).into(),
            te_state("tp-1", &state).into(),
        ],
    );
    assert_try_send_resumes(&admission, &[TrySend::Queued]);
    logs.assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="127.0.0.1:3001""#])
        .assert_and_remove(Level::DEBUG, &["perf.header.lifecycle", r#"outcome="invalid_header""#])
        .assert_and_remove(Level::ERROR, &["perf.header.lifecycle", "Unknown peer"])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_roll_forward_known_peer_header_already_stored() {
    let prep = test_prep();
    let peer = Peer::for_test(3001);
    let parent = &prep.headers[0];
    let header = &prep.headers[1];
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(header, EraName::Conway), header.point()),
    });

    let mut state = prep.state.clone();
    state.insert_peer(peer, prep.conn_id, parent.point(), parent.point());

    let mut expected = prep.state.clone();
    expected.insert_peer(peer, prep.conn_id, header.point(), header.point());

    let received_at = Instant::at_offset(Duration::from_secs(SIM_INITIAL_CLOCK_SECS), start_in_era().relative_time);
    let (running, _guards, mut logs) =
        setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store_with_nonces(slice::from_ref(header)));
    let admission = running.trace_buffer().lock().hydrate_without_timestamps();
    assert_trace_match(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_clock_suspend("tp-1").into(),
            tm_try_send("tp-1", "", RequestNext),
            te_get_nonces("tp-1", header.hash()).into(),
            te_record_header_announcement(
                "tp-1",
                peer,
                header.point(),
                header.parent_hash(),
                received_at,
                slot_start_to_header_micros(&header.point(), received_at),
                true,
            )
            .into(),
            te_state("tp-1", &expected).into(),
        ],
    );
    assert_try_send_resumes(&admission, &[TrySend::Queued]);
    logs.assert_and_remove(Level::DEBUG, &["chainsync.roll_forward_done", r#"outcome="already_stored""#])
        .assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="127.0.0.1:3001""#])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

/// The first three distinct peers to announce one header hash are logged, in arrival order.
/// The first peer stores the header. Later peers find it already stored and still fill ranks 2
/// and 3 while that list is open. A fourth announcer is recorded for peer selection and produces
/// no further propagation line.
#[test]
fn test_header_announcement_logs_the_first_three_peers_for_a_hash() {
    let prep = test_prep();
    let parent = &prep.headers[0];
    let header = &prep.headers[1];
    let mut state = prep.state.clone();
    let mut ids = ConnectionId::initial();
    let peers: Vec<(Peer, ConnectionId)> = (0..4)
        .map(|offset| {
            let peer = Peer::for_test(3001 + offset);
            let conn_id = ids.get_and_increment();
            state.insert_peer(peer, conn_id, parent.point(), parent.point());
            (peer, conn_id)
        })
        .collect();
    let msgs = peers.iter().map(|(peer, conn_id)| {
        TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
            peer: *peer,
            conn_id: *conn_id,
            handler: prep.handler.clone(),
            msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(header, EraName::Conway), header.point()),
        })
    });

    let (_running, _guards, mut logs) = setup_base(&prep.rt_handle(), state, msgs, build_store(&[]), |running| {
        running.override_external_effect::<ValidateHeaderEffect>(usize::MAX, |_| {
            OverrideResult::handled(Ok(Nonces::for_tests()))
        });
    });
    let hash = format!(r#"header_hash="{}""#, header.hash());
    logs.assert_and_remove(Level::DEBUG, &["header.announced", &hash, r#"peer="127.0.0.1:3001""#, "rank=1"])
        .assert_and_remove(Level::DEBUG, &["header.announced", &hash, r#"peer="127.0.0.1:3002""#, "rank=2"])
        .assert_and_remove(Level::DEBUG, &["header.announced", &hash, r#"peer="127.0.0.1:3003""#, "rank=3"]);
    let rest = logs.to_string();
    assert!(!rest.contains("header.announced"), "only three announcements:\n{rest}");
    assert!(!rest.contains("duplicate_header"), "an extra announcer is not a duplicate lifecycle:\n{rest}");
}

/// A header may already sit in the chain store without nonces (legacy import / incomplete
/// migration). When re-received from a peer, its nonces must still be computed so descendant
/// headers can be validated. Nonce absence means the header was never fully validated, so it is
/// treated like a new header: stored with its nonces and propagated downstream. `select_chain`
/// accepts the resulting tip even if concurrent recovery already validated the block body.
#[test]
fn test_roll_forward_stored_header_missing_nonces_revalidates() {
    let prep = test_prep();
    let peer = Peer::for_test(3001);
    let parent = &prep.headers[0];
    let header = &prep.headers[1];
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(header, EraName::Conway), header.point()),
    });

    let mut state = prep.state.clone();
    state.insert_peer(peer, prep.conn_id, parent.point(), parent.point());

    let mut expected = prep.state.clone();
    expected.insert_peer(peer, prep.conn_id, header.point(), header.point());

    // Header present but nonces absent, as after a bootstrap import.
    let received_at = Instant::at_offset(Duration::from_secs(SIM_INITIAL_CLOCK_SECS), start_in_era().relative_time);
    let (running, _guards, mut logs) =
        setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(slice::from_ref(header)));
    let admission = running.trace_buffer().lock().hydrate_without_timestamps();
    assert_trace_contains(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_clock_suspend("tp-1").into(),
            tm_try_send("tp-1", "", RequestNext),
            te_get_nonces("tp-1", header.hash()).into(),
            te_validate_header("tp-1", header.clone()).into(),
            te_store_validated_header("tp-1", header.clone()).into(),
            te_record_header_announcement(
                "tp-1",
                peer,
                header.point(),
                header.parent_hash(),
                received_at,
                slot_start_to_header_micros(&header.point(), received_at),
                false,
            )
            .into(),
            te_send("tp-1", "downstream", new_tip(header.point(), parent.point())).into(),
            te_state("tp-1", &expected).into(),
        ],
    );
    assert_try_send_resumes(&admission, &[TrySend::Queued]);
    logs.assert_and_remove(Level::DEBUG, &["chainsync.roll_forward_done", r#"outcome="stored""#])
        .assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="127.0.0.1:3001""#])
        .assert_and_remove(
            Level::DEBUG,
            &[
                "amaru::blockperf",
                "header.announced",
                r#"peer="127.0.0.1:3001""#,
                "rank=1",
                &format!(r#"header_hash="{}""#, header.hash()),
            ],
        )
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_roll_forward_known_peer_new_header_forwards_tip() {
    let prep = test_prep();
    let peer = Peer::for_test(3001);
    let parent = &prep.headers[0];
    let header = &prep.headers[1];
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(header, EraName::Conway), header.point()),
    });

    let mut state = prep.state.clone();
    state.insert_peer(peer, prep.conn_id, parent.point(), parent.point());

    let mut expected = prep.state.clone();
    expected.insert_peer(peer, prep.conn_id, header.point(), header.point());

    let received_at = Instant::at_offset(Duration::from_secs(SIM_INITIAL_CLOCK_SECS), start_in_era().relative_time);
    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    let admission = running.trace_buffer().lock().hydrate_without_timestamps();
    assert_trace_contains(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_clock_suspend("tp-1").into(),
            tm_try_send("tp-1", "", RequestNext),
            te_get_nonces("tp-1", header.hash()).into(),
            te_validate_header("tp-1", header.clone()).into(),
            te_store_validated_header("tp-1", header.clone()).into(),
            te_record_header_announcement(
                "tp-1",
                peer,
                header.point(),
                header.parent_hash(),
                received_at,
                slot_start_to_header_micros(&header.point(), received_at),
                false,
            )
            .into(),
            te_send("tp-1", "downstream", new_tip(header.point(), parent.point())).into(),
            te_state("tp-1", &expected).into(),
        ],
    );
    assert_try_send_resumes(&admission, &[TrySend::Queued]);
    logs.assert_and_remove(Level::DEBUG, &["chainsync.roll_forward_done", r#"outcome="stored""#])
        .assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="127.0.0.1:3001""#])
        .assert_and_remove(
            Level::DEBUG,
            &[
                "amaru::blockperf",
                "header.announced",
                r#"peer="127.0.0.1:3001""#,
                "rank=1",
                &format!(r#"header_hash="{}""#, header.hash()),
            ],
        )
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

/// Consecutive headers may skip empty Praos slots; a later slot that is still in the past is valid.
#[test]
fn test_roll_forward_accepts_empty_slots_in_the_past() {
    let prep = test_prep();
    let peer = Peer::for_test(3001);
    let parent = &prep.headers[0];
    let header = make_block_header(2, parent.slot().as_u64() + 130, Some(parent.hash()));
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&header, EraName::Conway), header.point()),
    });

    let mut state = prep.state.clone();
    state.insert_peer(peer, prep.conn_id, parent.point(), parent.point());

    let mut expected = prep.state.clone();
    expected.insert_peer(peer, prep.conn_id, header.point(), header.point());

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    let admission = running.trace_buffer().lock().hydrate_without_timestamps();
    assert_trace_contains(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_clock_suspend("tp-1").into(),
            tm_try_send("tp-1", "", RequestNext),
            te_validate_header("tp-1", header.clone()).into(),
            te_store_validated_header("tp-1", header.clone()).into(),
            te_send("tp-1", "downstream", new_tip(header.point(), parent.point())).into(),
            te_state("tp-1", &expected).into(),
        ],
    );
    assert_try_send_resumes(&admission, &[TrySend::Queued]);
    assert_trace_does_not_contain(&running, &[tm_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer))]);
    logs.assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="127.0.0.1:3001""#])
        .assert_and_remove(Level::DEBUG, &["chainsync.roll_forward_done", r#"outcome="stored""#])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_roll_forward_invalid_variant_removes_peer() {
    let prep = test_prep();
    let peer = Peer::for_test(3001);
    let parent = &prep.headers[0];
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(
            HeaderContent::with_bytes(vec![], EraName::Babbage),
            parent.point(),
        ),
    });

    let expected = prep.state.clone();
    let mut state = prep.state.clone();
    state.insert_peer(peer, prep.conn_id, parent.point(), parent.point());

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
    let peer = Peer::for_test(3001);
    let parent = &prep.headers[0];
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(
            HeaderContent::with_bytes(vec![0xff], EraName::Conway),
            parent.point(),
        ),
    });

    let expected = prep.state.clone();
    let mut state = prep.state.clone();
    state.insert_peer(peer, prep.conn_id, parent.point(), parent.point());

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
    let peer = Peer::for_test(3001);
    let parent = &prep.headers[0];
    let wrong_parent = HeaderHash::from([9u8; 32]);
    let header = make_block_header(2, 2, Some(wrong_parent));
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&header, EraName::Conway), header.point()),
    });

    let expected = prep.state.clone();
    let mut state = prep.state.clone();
    state.insert_peer(peer, prep.conn_id, parent.point(), parent.point());

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    let admission = running.trace_buffer().lock().hydrate_without_timestamps();
    assert_trace_match(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_clock_suspend("tp-1").into(),
            tm_try_send("tp-1", "", RequestNext),
            te_header_rejected("invalid header").into(),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer)).into(),
            te_state("tp-1", &expected).into(),
        ],
    );
    assert_try_send_resumes(&admission, &[TrySend::Queued]);
    logs.assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="127.0.0.1:3001""#])
        .assert_and_remove(Level::DEBUG, &["perf.header.lifecycle", r#"outcome="invalid_header""#])
        .assert_and_remove(Level::ERROR, &["perf.header.lifecycle", "Invalid header parent"])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_roll_forward_invalid_height_removes_peer() {
    let prep = test_prep();
    let peer = Peer::for_test(3001);
    let parent = &prep.headers[0];
    let header = make_block_header(3, 2, Some(parent.hash()));
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&header, EraName::Conway), header.point()),
    });

    let expected = prep.state.clone();
    let mut state = prep.state.clone();
    state.insert_peer(peer, prep.conn_id, parent.point(), parent.point());

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    let admission = running.trace_buffer().lock().hydrate_without_timestamps();
    assert_trace_match(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_clock_suspend("tp-1").into(),
            tm_try_send("tp-1", "", RequestNext),
            te_header_rejected("invalid header").into(),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer)).into(),
            te_state("tp-1", &expected).into(),
        ],
    );
    assert_try_send_resumes(&admission, &[TrySend::Queued]);
    logs.assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="127.0.0.1:3001""#])
        .assert_and_remove(Level::DEBUG, &["perf.header.lifecycle", r#"outcome="invalid_header""#])
        .assert_and_remove(Level::ERROR, &["perf.header.lifecycle", "Invalid header height"])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_roll_forward_invalid_point_removes_peer() {
    let prep = test_prep();
    let peer = Peer::for_test(3001);
    let parent = &prep.headers[0];
    let header = make_block_header(2, parent.slot().into(), Some(parent.hash()));
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&header, EraName::Conway), parent.point()),
    });

    let expected = prep.state.clone();
    let mut state = prep.state.clone();
    state.insert_peer(peer, prep.conn_id, parent.point(), parent.point());

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    let admission = running.trace_buffer().lock().hydrate_without_timestamps();
    assert_trace_match(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_clock_suspend("tp-1").into(),
            tm_try_send("tp-1", "", RequestNext),
            te_header_rejected("invalid header").into(),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer)).into(),
            te_state("tp-1", &expected).into(),
        ],
    );
    assert_try_send_resumes(&admission, &[TrySend::Queued]);
    logs.assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="127.0.0.1:3001""#])
        .assert_and_remove(Level::DEBUG, &["perf.header.lifecycle", r#"outcome="invalid_header""#])
        .assert_and_remove(Level::ERROR, &["perf.header.lifecycle", "Invalid header point"])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_roll_forward_header_validation_failure_removes_peer() {
    let prep = test_prep();
    let peer = Peer::for_test(3001);
    let parent = &prep.headers[0];
    let header = &prep.headers[1];
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(header, EraName::Conway), header.point()),
    });

    let expected = prep.state.clone();
    let mut state = prep.state.clone();
    state.insert_peer(peer, prep.conn_id, parent.point(), header.point());

    // Use empty store so evolve_nonce fails (unknown parent), exercising the real validate_header fn failure path.
    let (running, _guards, mut logs) =
        setup_base(&prep.rt_handle(), state.clone(), [msg.clone()], build_store(&[]), |running| {
            let header = header.hash();
            let parent = parent.hash();
            running.override_external_effect::<ValidateHeaderEffect>(usize::MAX, move |_| {
                OverrideResult::handled(Err(ValidateHeaderError::Nonces(NoncesError::UnknownParent { header, parent })))
            });
        });

    logs.assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="127.0.0.1:3001""#])
        .assert_and_remove(Level::DEBUG, &["perf.header.lifecycle", r#"outcome="invalid_header""#])
        .assert_and_remove(Level::ERROR, &["perf.header.lifecycle"])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
    let admission = running.trace_buffer().lock().hydrate_without_timestamps();
    assert_trace_match(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_clock_suspend("tp-1").into(),
            tm_try_send("tp-1", "", RequestNext),
            te_get_nonces("tp-1", header.hash()).into(),
            te_validate_header("tp-1", header.clone()).into(),
            te_header_rejected("invalid header").into(),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer)).into(),
            te_state("tp-1", &expected).into(),
        ],
    );
    assert_try_send_resumes(&admission, &[TrySend::Queued]);
}

/// Header onset more than two seconds ahead of sim clock → adversarial.
/// Slot math must use the same `EraHistory` as `TrackPeers` (`EraHistory::default()` in tests).
#[test]
fn test_roll_forward_header_slot_too_far_future_adversarial() {
    let prep = test_prep();
    let peer = Peer::for_test(3001);
    let elapsed = prep.start_times.relative_time + Duration::from_secs(10);
    let curr_slot = EraHistory::default().relative_time_to_slot(elapsed).expect("slot from start time").as_u64();
    // Parent at "now" so the header is within the foreseeable horizon of `current`.
    let parent = make_block_header(1, curr_slot, None);
    let header = make_block_header(2, curr_slot + 10, Some(parent.hash()));
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&header, EraName::Conway), header.point()),
    });

    let now = Instant::at_offset(Duration::from_secs(SIM_INITIAL_CLOCK_SECS), start_in_era().relative_time);
    let mut expected = prep.state.clone();
    expected.last_chain_lag_check = Some(now);
    expected.chain_lag = Some(ChainLagSample {
        at: now,
        lateness: tip_lateness(Point::Origin.slot(), now, &EraHistory::default()).expect("origin slot"),
    });
    let mut state = prep.state.clone();
    state.insert_peer(peer, prep.conn_id, parent.point(), header.point());

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));

    logs.assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="127.0.0.1:3001""#])
        .assert_and_remove(Level::DEBUG, &["perf.header.lifecycle", r#"outcome="invalid_header""#])
        .assert_and_remove(Level::ERROR, &["perf.header.lifecycle", "ahead of local time"])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
    let admission = running.trace_buffer().lock().hydrate_without_timestamps();
    assert_trace_match(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_clock_suspend("tp-1").into(),
            te_get_best_chain_tip("tp-1").into(),
            te_sync_adoption_is_fast("tp-1", now).into(),
            tm_try_send("tp-1", "", RequestNext),
            te_header_rejected("invalid header").into(),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer)).into(),
            te_state("tp-1", &expected).into(),
        ],
    );
    assert_try_send_resumes(&admission, &[TrySend::Queued]);
}

/// A slot beyond the foreseeable horizon (`EraHistoryError::PastTimeHorizon`) is adversarial.
#[test]
fn test_roll_forward_slot_past_time_horizon_is_adversarial() {
    let prep = test_prep();
    let peer = Peer::for_test(3001);
    let parent = &prep.headers[0];
    // Default test era rounds the stability window up to a full epoch (86400 slots).
    let header = make_block_header(2, parent.slot().as_u64() + 100_000, Some(parent.hash()));
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&header, EraName::Conway), header.point()),
    });

    let expected = prep.state.clone();
    let mut state = prep.state.clone();
    state.insert_peer(peer, prep.conn_id, parent.point(), parent.point());

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    let admission = running.trace_buffer().lock().hydrate_without_timestamps();
    assert_trace_match(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_clock_suspend("tp-1").into(),
            tm_try_send("tp-1", "", RequestNext),
            te_header_rejected("invalid header").into(),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer)).into(),
            te_state("tp-1", &expected).into(),
        ],
    );
    assert_try_send_resumes(&admission, &[TrySend::Queued]);
    logs.assert_and_remove(Level::ERROR, &["perf.header.lifecycle", "past time horizon"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

/// Header onset 1–2s ahead of sim clock → clock-skew defer (not adversarial).
#[test]
fn test_roll_forward_header_slot_near_future_defers() {
    let prep = test_prep();
    let peer = Peer::for_test(3001);
    let elapsed = prep.start_times.relative_time + Duration::from_secs(10);
    let curr_slot = EraHistory::default().relative_time_to_slot(elapsed).expect("slot from start time").as_u64();
    let parent = make_block_header(1, curr_slot, None);
    // one second ahead of sim clock → near-future defer
    let header = make_block_header(2, curr_slot + 1, Some(parent.hash()));
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&header, EraName::Conway), header.point()),
    });

    let mut state = prep.state.clone();
    state.insert_peer(peer, prep.conn_id, parent.point(), header.point());

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));

    logs.assert_and_remove(Level::DEBUG, &["chainsync.header_deferred", r#"reason="clock_skew""#])
        .assert_and_remove(Level::DEBUG, &["chainsync.roll_forward_done", r#"outcome="stored""#])
        .assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="127.0.0.1:3001""#])
        .assert_and_remove(
            Level::DEBUG,
            &["header.announced", r#"peer="127.0.0.1:3001""#, "rank=1", &format!(r#"header_hash="{}""#, header.hash())],
        )
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
    // Clock-skew defers, then sim advances and RecheckLedgerHeight processes the header.
    let admission = running.trace_buffer().lock().hydrate_without_timestamps();
    assert_trace_contains(
        &running,
        &[
            te_input("tp-1", &msg).into(),
            te_clock_suspend("tp-1").into(),
            tm_try_send("tp-1", "", RequestNext),
            te_clock_suspend("tp-1").into(),
            tm_state::<TrackPeers>("tp-1", |s| s.deferred.len() == 1, "clock skew deferred"),
            te_input("tp-1", &TrackPeersMsg::RecheckLedgerHeight).into(),
            te_validate_header("tp-1", header.clone()).into(),
            te_store_validated_header("tp-1", header.clone()).into(),
            tm_state::<TrackPeers>("tp-1", |s| s.deferred.is_empty(), "processed after recheck"),
        ],
    );
    assert_try_send_resumes(&admission, &[TrySend::Queued]);
    assert_trace_does_not_contain(&running, &[tm_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer))]);
}

/// Tests that a header whose required stake distribution is more than 1 epoch ahead
/// causes immediate adversarial rejection (no deferral).
#[test]
fn test_roll_forward_stake_dist_far_ahead_rejects() {
    let prep = test_prep();
    let peer = Peer::for_test(3001);
    let parent = &prep.headers[0];
    let header = &prep.headers[1];
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(header, EraName::Conway), header.point()),
    });

    let expected = prep.state.clone();
    let mut state = prep.state.clone();
    state.insert_peer(peer, prep.conn_id, parent.point(), header.point());

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

    logs.assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="127.0.0.1:3001""#])
        .assert_and_remove(Level::DEBUG, &["perf.header.lifecycle", r#"outcome="invalid_header""#])
        .assert_and_remove(Level::ERROR, &["perf.header.lifecycle"])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
    let admission = running.trace_buffer().lock().hydrate_without_timestamps();
    assert_trace_match(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            te_clock_suspend("tp-1").into(),
            tm_try_send("tp-1", "", RequestNext),
            te_get_nonces("tp-1", header.hash()).into(),
            te_validate_header("tp-1", header.clone()).into(),
            te_header_rejected("invalid header").into(),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer)).into(),
            te_state("tp-1", &expected).into(),
        ],
    );
    assert_try_send_resumes(&admission, &[TrySend::Queued]);
}

#[test]
fn test_roll_backward_updates_peer() {
    let prep = test_prep();
    let peer = Peer::for_test(3001);
    let header = &prep.headers[0];
    let current = header.point();
    let tip = current;
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollBackward(current, tip),
    });

    let mut state = prep.state.clone();
    state.insert_peer(peer, prep.conn_id, Point::Origin, Point::Origin);

    let mut expected = prep.state.clone();
    expected.insert_peer(peer, prep.conn_id, header.point(), tip);

    let now = Instant::at_offset(Duration::from_secs(SIM_INITIAL_CLOCK_SECS), start_in_era().relative_time);
    let (running, _guards, mut logs) =
        setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(slice::from_ref(header)));
    let admission = running.trace_buffer().lock().hydrate_without_timestamps();
    assert_trace_contains(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            tm_try_send("tp-1", "", RequestNext),
            te_load_point("tp-1", current.hash()).into(),
            te_load_header("tp-1", current.hash()).into(),
            te_clock_read("tp-1").into(),
            te_record_rollback("tp-1", peer, header.point(), header.parent_hash(), now).into(),
            te_state("tp-1", &expected).into(),
        ],
    );
    assert_try_send_resumes(&admission, &[TrySend::Queued]);
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
    let peer = Peer::for_test(3001);
    let header = &prep.headers[0];
    let current = header.point();
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollBackward(current, Point::Origin),
    });

    let state = prep.state.clone();

    let (running, _guards, mut logs) =
        setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(slice::from_ref(header)));
    let admission = running.trace_buffer().lock().hydrate_without_timestamps();
    assert_trace_contains(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            tm_try_send("tp-1", "", RequestNext),
            te_load_point("tp-1", current.hash()).into(),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer)).into(),
            te_state("tp-1", &state).into(),
        ],
    );
    assert_try_send_resumes(&admission, &[TrySend::Queued]);
    logs.assert_and_remove(Level::ERROR, &["chainsync.roll_backward_failed", "Unknown peer"])
        .assert_and_remove(Level::INFO, &["chainsync.roll_backward"])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_roll_backward_unknown_point_removes_peer() {
    let prep = test_prep();
    let peer = Peer::for_test(3001);
    let current = Point::Specific(1u64.into(), HeaderHash::from([1u8; 32]), BlockHeight::from(1));
    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollBackward(current, Point::Origin),
    });

    let expected = prep.state.clone();
    let mut state = prep.state.clone();
    state.insert_peer(peer, prep.conn_id, Point::Origin, Point::Origin);

    let (running, _guards, mut logs) = setup(&prep.rt_handle(), state.clone(), msg.clone(), build_store(&[]));
    let admission = running.trace_buffer().lock().hydrate_without_timestamps();
    assert_trace_contains(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg).into(),
            tm_try_send("tp-1", "", RequestNext),
            te_load_point("tp-1", current.hash()).into(),
            te_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer)).into(),
            te_state("tp-1", &expected).into(),
        ],
    );
    assert_try_send_resumes(&admission, &[TrySend::Queued]);
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
    let peer = Peer::for_test(3001);
    let header = prep.headers[0].clone();
    let tip = header.point();

    let mut state = prep.state.clone();
    state.insert_peer(peer, prep.conn_id, Point::Origin, tip);

    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
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
    .assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="127.0.0.1:3001""#])
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
    assert_trace_does_not_contain(&running, &[tm_any_request_next()]);
}

#[test]
fn test_pipelined_headers_after_height_defer() {
    let prep = test_prep_with_max_peer_lead(0);
    let peer = Peer::for_test(3001);
    let parent = &prep.headers[0];
    let h1 = prep.headers[1].clone();
    let h2 = make_block_header(3, h1.slot().as_u64() + 1, Some(h1.hash()));

    let msg1 = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&h1, EraName::Conway), h1.point()),
    });
    let msg2 = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&h2, EraName::Conway), h2.point()),
    });

    let mut state = prep.state.clone();
    state.insert_peer(peer, prep.conn_id, parent.point(), h2.point());

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
        .assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="127.0.0.1:3001""#])
        .assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="127.0.0.1:3001""#])
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
    assert_trace_does_not_contain(&running, &[tm_any_request_next()]);
}

/// Height defer is released when a later recheck sees the applied ledger height advance.
#[test]
fn test_height_defer_recheck_when_ledger_advances() {
    let prep = test_prep_with_max_peer_lead(0);
    let peer = Peer::for_test(3001);
    let header = prep.headers[0].clone();
    let tip = header.point();

    let mut state = prep.state.clone();
    state.insert_peer(peer, prep.conn_id, Point::Origin, tip);

    let msg = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
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
        .assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="127.0.0.1:3001""#])
        .assert_and_remove(
            Level::DEBUG,
            &["header.announced", r#"peer="127.0.0.1:3001""#, "rank=1", &format!(r#"header_hash="{}""#, header.hash())],
        )
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);

    let admission = running.trace_buffer().lock().hydrate_without_timestamps();
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
            tm_try_send("tp-1", "", RequestNext),
            tm_state::<TrackPeers>(
                "tp-1",
                |s| s.deferred.is_empty() && s.recheck_timer.is_none(),
                "processed after height advanced",
            ),
        ],
    );
    assert_try_send_resumes(&admission, &[TrySend::Queued]);
}

#[test]
fn test_pipelined_headers_after_slot_near_future_defer() {
    let prep = test_prep();
    let peer = Peer::for_test(3001);
    let elapsed = prep.start_times.relative_time + Duration::from_secs(10);
    let curr_slot = EraHistory::default().relative_time_to_slot(elapsed).expect("slot from start time").as_u64();
    let parent = make_block_header(1, curr_slot, None);
    // first is near-future; second is FollowUp while peer deferred
    let h1 = make_block_header(2, curr_slot + 1, Some(parent.hash()));
    let h2 = make_block_header(3, curr_slot + 2, Some(h1.hash()));

    let msg1 = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&h1, EraName::Conway), h1.point()),
    });
    let msg2 = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&h2, EraName::Conway), h2.point()),
    });

    let mut state = prep.state.clone();
    state.insert_peer(peer, prep.conn_id, parent.point(), h2.point());

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
        .assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="127.0.0.1:3001""#])
        .assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="127.0.0.1:3001""#])
        .assert_and_remove(Level::DEBUG, &["header.announced", &format!(r#"header_hash="{h1_hash}""#), "rank=1"])
        .assert_and_remove(Level::DEBUG, &["header.announced", &format!(r#"header_hash="{h2_hash}""#), "rank=1"])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
    // First header clock-skew defers; second is FollowUp; recheck may drain both before run ends.
    // Both headers ask the blackhole for the next header, and both are admitted.
    let admission = running.trace_buffer().lock().hydrate_without_timestamps();
    assert_trace_contains(
        &running,
        &[
            te_input("tp-1", &msg1).into(),
            tm_try_send("tp-1", "", RequestNext),
            tm_state::<TrackPeers>("tp-1", |s| s.deferred.len() == 1, "first clock-skew deferred"),
            te_input("tp-1", &msg2).into(),
            tm_state::<TrackPeers>("tp-1", |s| s.deferred.len() == 2, "follow-up queued while deferred"),
            tm_try_send("tp-1", "", RequestNext),
        ],
    );
    assert_try_send_resumes(&admission, &[TrySend::Queued, TrySend::Queued]);
    assert_trace_does_not_contain(&running, &[tm_send("tp-1", "peer_selection", PeerSelectionMsg::adversarial(peer))]);
}

/// Pipelined stake dist not available: multiple headers arrive (from pipelining), both defer,
/// then StakeDistUpdated wakes them for sequential re-validation and processing.
#[test]
fn test_pipelined_stake_defer_and_wake_sequence() {
    let prep = test_prep();
    let peer = Peer::for_test(3001);
    let parent = &prep.headers[0];
    let h1 = prep.headers[1].clone();
    let h2 = make_block_header(3, h1.slot().as_u64() + 1, Some(h1.hash()));

    let msg1 = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&h1, EraName::Conway), h1.point()),
    });
    let msg2 = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&h2, EraName::Conway), h2.point()),
    });
    // Advance max_epoch far enough that the previously missing target epoch is covered.
    let wake = TrackPeersMsg::StakeDistUpdated(prep.start_times.epoch);

    let mut state = prep.state.clone();
    state.insert_peer(peer, prep.conn_id, parent.point(), h2.point());

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
        .assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="127.0.0.1:3001""#])
        .assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="127.0.0.1:3001""#])
        .assert_and_remove(Level::DEBUG, &["header.announced", &format!(r#"header_hash="{h1_hash}""#), "rank=1"])
        .assert_and_remove(Level::DEBUG, &["header.announced", &format!(r#"header_hash="{h2_hash}""#), "rank=1"])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
    // h1 stake-deferred after RN; h2 is FollowUp (peer already deferred); wake reprocesses both in order.
    let admission = running.trace_buffer().lock().hydrate_without_timestamps();
    assert_trace_contains(
        &running,
        &[
            te_state("tp-1", &state).into(),
            te_input("tp-1", &msg1).into(),
            te_clock_suspend("tp-1").into(),
            tm_try_send("tp-1", "", RequestNext),
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
            tm_try_send("tp-1", "", RequestNext),
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
    assert_try_send_resumes(&admission, &[TrySend::Queued, TrySend::Queued]);
}

/// Two headers deferred for the same connection; on recheck the first fails validation and
/// purges the connection, which also drops the second entry from the deferred list.
/// Regression: the recheck loop used to index past the shrunk list and panic.
#[test]
fn test_recheck_deferred_survives_purge_shrinking_the_list() {
    let prep = test_prep();
    let peer = Peer::for_test(3001);
    let parent = &prep.headers[0];
    let wrong_parent = HeaderHash::from([9u8; 32]);
    let h1 = make_block_header(2, 2, Some(wrong_parent));
    let h2 = make_block_header(3, 3, Some(h1.hash()));

    let mut state = prep.state.clone();
    state.insert_peer(peer, prep.conn_id, parent.point(), parent.point());
    state.push_deferred_for_tests(peer, prep.conn_id, prep.handler.clone(), h1.clone(), h1.point());
    state.push_deferred_for_tests(peer, prep.conn_id, prep.handler.clone(), h2.clone(), h2.point());

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
    let peer = Peer::for_test(3001);
    let parent = &prep.headers[0];
    let h1 = prep.headers[1].clone();
    let h2 = make_block_header(3, h1.slot().as_u64() + 1, Some(h1.hash()));

    let msg1 = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
        conn_id: prep.conn_id,
        handler: prep.handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(&h1, EraName::Conway), h1.point()),
    });
    let msg2 = TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
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
    state.insert_peer(peer, prep.conn_id, parent.point(), h2.point());

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
    .assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="127.0.0.1:3001""#])
    .assert_and_remove(Level::DEBUG, &["roll_forward.process", r#"peer="127.0.0.1:3001""#])
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

fn linked_headers(len: u64) -> Vec<Header> {
    let mut headers = vec![make_block_header(1, 1, None)];
    for n in 1..len {
        let parent = headers.last().expect("chain").hash();
        headers.push(make_block_header(n + 1, n + 1, Some(parent)));
    }
    headers
}

fn roll_forward_msg(
    peer: Peer,
    conn_id: ConnectionId,
    handler: &StageRef<InitiatorMessage>,
    header: &Header,
) -> TrackPeersMsg {
    TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
        peer,
        conn_id,
        handler: handler.clone(),
        msg: chainsync::InitiatorResult::RollForward(HeaderContent::new(header, EraName::Conway), header.point()),
    })
}

fn owed(state: &TrackPeers, conn_id: ConnectionId) -> Option<u8> {
    match state.upstream.get(&conn_id) {
        Some(super::PerPeer::Established { owed, .. }) => Some(*owed),
        Some(super::PerPeer::Connecting { .. }) | None => None,
    }
}

fn current_point(state: &TrackPeers, conn_id: ConnectionId) -> Option<Point> {
    match state.upstream.get(&conn_id) {
        Some(super::PerPeer::Established { current, .. }) => Some(*current),
        Some(super::PerPeer::Connecting { .. }) | None => None,
    }
}

fn park_handler_full(
    running: &mut amaru_pure_stage::simulation::SimulationRunning,
    handler: &impl AsRef<StageRef<InitiatorMessage>>,
) -> Instant {
    running.enqueue_msg(handler, [RequestNext]);
    let parked = running.run(Run::default()).assert_sleeping();
    for _ in 0..DEFAULT_MAILBOX_SIZE {
        running.enqueue_msg(handler, [RequestNext]);
    }
    assert_eq!(running.mailbox_len(handler), DEFAULT_MAILBOX_SIZE);
    parked
}

/// Run until the next wakeup or idle, resolving external effects without skipping the wakeup.
fn drive(running: &mut amaru_pure_stage::simulation::SimulationRunning, rt: &tokio::runtime::Handle) -> Blocked {
    loop {
        match running.run(Run::default()) {
            Blocked::Busy { .. } => {
                rt.block_on(running.await_external_effect());
            }
            blocked @ (Blocked::Idle
            | Blocked::Sleeping { .. }
            | Blocked::Deadlock(_)
            | Blocked::Breakpoint(_)
            | Blocked::Terminated(_)) => return blocked,
        }
    }
}

fn tm_retry_timeout() -> amaru_pure_stage::TraceMatch<'static> {
    amaru_pure_stage::TraceMatch::Property(
        Box::new(|src| {
            matches!(
                src.suspend(),
                Some(Effect::SetTimeout { slot, delay, msg, .. })
                    if *slot == super::REQUEST_RETRY_SLOT
                        && *delay == super::REQUEST_RETRY_DELAY
                        && msg
                            .cast_ref::<TrackPeersMsg>()
                            .is_ok_and(|message| matches!(message, TrackPeersMsg::RetryRequestNext))
            )
        }),
        "owed RequestNext retry timeout".to_string(),
    )
}

fn not_try_send() -> amaru_pure_stage::TraceMatch<'static> {
    amaru_pure_stage::TraceMatch::Property(
        Box::new(|src| !matches!(src.suspend(), Some(Effect::TrySend { .. }))),
        "not a try_send".to_string(),
    )
}

/// The admission result is the resume of `tp-1`, not the `TrySend` effect.
fn assert_try_send_resumes(trace: &[TraceEntry], outcomes: &[TrySend]) {
    let got: Vec<TrySend> = trace
        .iter()
        .filter_map(|entry| match entry {
            TraceEntry::Resume { stage, response: StageResponse::TrySend(outcome) } if stage.as_str() == "tp-1" => {
                Some(*outcome)
            }
            TraceEntry::Resume { .. }
            | TraceEntry::Suspend(_)
            | TraceEntry::Clock(_)
            | TraceEntry::Input { .. }
            | TraceEntry::State { .. }
            | TraceEntry::Terminated { .. }
            | TraceEntry::InvalidBytes(..) => None,
        })
        .collect();
    assert_eq!(got, outcomes, "try_send responses missing or reordered: {trace:?}");
}

fn not_admission() -> amaru_pure_stage::TraceMatch<'static> {
    amaru_pure_stage::TraceMatch::Property(
        Box::new(|src| {
            !matches!(
                src.suspend(),
                Some(Effect::TrySend { .. } | Effect::SetTimeout { .. } | Effect::ClearTimeout { .. })
            )
        }),
        "not an admission or timeout".to_string(),
    )
}

struct Ready {
    rt: tokio::runtime::Runtime,
    state: TrackPeers,
    peer: Peer,
    conn_id: ConnectionId,
}

fn ready_peer(headers: &[Header]) -> Ready {
    let prep = test_prep();
    let peer = Peer::for_test(3001);
    let mut state = prep.state;
    state.insert_peer(peer, prep.conn_id, headers[0].point(), headers[0].point());
    Ready { rt: prep.rt, state, peer, conn_id: prep.conn_id }
}

/// A new `RequestNext` that does not fit counts one miss and leaves a `TrySend::Full` in the trace.
#[test]
fn full_on_a_new_request_counts_one() {
    let headers = linked_headers(2);
    let ready = ready_peer(&headers);
    let mut opened =
        open_fanout(ready.rt.handle(), ready.state, build_store(slice::from_ref(&headers[0])), HandlerHold::Hour);
    park_handler_full(&mut opened.running, &opened.handler);
    opened.running.trace_buffer().lock().clear();
    opened.running.enqueue_msg(&opened.tp, [roll_forward_msg(ready.peer, ready.conn_id, &opened.handler, &headers[1])]);
    let retry_at = drive(&mut opened.running, ready.rt.handle()).assert_sleeping();
    assert_eq!(retry_at.saturating_since(opened.running.now()), super::REQUEST_RETRY_DELAY);

    let state = opened.running.get_state(&opened.tp).expect("track_peers idle");
    assert_eq!(owed(state, ready.conn_id), Some(1));
    assert!(state.request_retry_armed);
    let admission = opened.running.trace_buffer().lock().hydrate_without_timestamps();
    assert_trace_contains(&opened.running, &[tm_try_send("tp-1", "handler", RequestNext), tm_retry_timeout()]);
    assert_try_send_resumes(&admission, &[TrySend::Full]);
}

/// Retrying a slot that is already counted does not count it again.
#[test]
fn full_retry_of_a_counted_slot_stays_at_one() {
    let headers = linked_headers(2);
    let ready = ready_peer(&headers);
    let mut opened =
        open_fanout(ready.rt.handle(), ready.state, build_store(slice::from_ref(&headers[0])), HandlerHold::Hour);
    park_handler_full(&mut opened.running, &opened.handler);
    opened.running.enqueue_msg(&opened.tp, [roll_forward_msg(ready.peer, ready.conn_id, &opened.handler, &headers[1])]);
    let retry_at = drive(&mut opened.running, ready.rt.handle()).assert_sleeping();
    opened.running.trace_buffer().lock().clear();
    opened.running.run(Run::until(retry_at));

    let state = opened.running.get_state(&opened.tp).expect("track_peers idle");
    assert_eq!(owed(state, ready.conn_id), Some(1));
    let admission = opened.running.trace_buffer().lock().hydrate_without_timestamps();
    assert_trace_match_filter(
        &opened.running,
        &[tm_try_send("tp-1", "handler", RequestNext), tm_retry_timeout()],
        &[not_admission()],
    );
    assert_try_send_resumes(&admission, &[TrySend::Full]);
}

/// A retry the handler accepts clears the one owed slot and offers exactly one `RequestNext`.
#[test]
fn queued_retry_clears_the_counter_with_one_request_next() {
    let headers = linked_headers(2);
    let ready = ready_peer(&headers);
    let mut opened = open_fanout(
        ready.rt.handle(),
        ready.state,
        build_store(slice::from_ref(&headers[0])),
        HandlerHold::FirstMillis,
    );
    let parked = park_handler_full(&mut opened.running, &opened.handler);
    opened.running.enqueue_msg(&opened.tp, [roll_forward_msg(ready.peer, ready.conn_id, &opened.handler, &headers[1])]);
    drive(&mut opened.running, ready.rt.handle()).assert_sleeping();
    assert_eq!(owed(opened.running.get_state(&opened.tp).expect("idle"), ready.conn_id), Some(1));

    opened.running.run(Run::until(parked)).assert_sleeping();
    opened.running.trace_buffer().lock().clear();
    let retry_at = opened.running.run(Run::default()).assert_sleeping();
    opened.running.run(Run::until(retry_at));

    let state = opened.running.get_state(&opened.tp).expect("track_peers idle");
    assert_eq!(owed(state, ready.conn_id), Some(0));
    assert!(!state.request_retry_armed);
    let admission = opened.running.trace_buffer().lock().hydrate_without_timestamps();
    assert_trace_match_filter(&opened.running, &[tm_try_send("tp-1", "handler", RequestNext)], &[not_try_send()]);
    assert_try_send_resumes(&admission, &[TrySend::Queued]);
}

/// Further misses once the counter is at the pipeline depth are not counted.
#[test]
fn owed_requests_saturate_at_pipeline_depth() {
    let headers = linked_headers(u64::from(PIPELINE_DEPTH) + 2);
    let ready = ready_peer(&headers);
    let mut opened =
        open_fanout(ready.rt.handle(), ready.state, build_store(slice::from_ref(&headers[0])), HandlerHold::Hour);
    park_handler_full(&mut opened.running, &opened.handler);
    for header in headers.iter().skip(1) {
        opened.running.enqueue_msg(&opened.tp, [roll_forward_msg(ready.peer, ready.conn_id, &opened.handler, header)]);
        drive(&mut opened.running, ready.rt.handle()).assert_sleeping();
    }
    let state = opened.running.get_state(&opened.tp).expect("track_peers idle");
    assert_eq!(owed(state, ready.conn_id), Some(PIPELINE_DEPTH));
    assert_eq!(current_point(state, ready.conn_id), Some(headers.last().expect("tip").point()));
}

/// A full handler does not stop header processing for a different peer.
#[test]
fn other_peer_keeps_moving_while_one_handler_is_full() {
    let headers = linked_headers(2);
    let prep = test_prep();
    let peer_a = Peer::for_test(3001);
    let peer_b = Peer::for_test(3002);
    let mut ids = ConnectionId::initial();
    let conn_a = ids.get_and_increment();
    let conn_b = ids.get_and_increment();
    let mut state = prep.state;
    state.insert_peer(peer_a, conn_a, headers[0].point(), headers[0].point());
    state.insert_peer(peer_b, conn_b, headers[0].point(), headers[0].point());

    let mut opened = open_fanout(prep.rt.handle(), state, build_store(slice::from_ref(&headers[0])), HandlerHold::Hour);
    park_handler_full(&mut opened.running, &opened.handler);
    let handler_b = StageRef::<InitiatorMessage>::blackhole();
    opened.running.enqueue_msg(
        &opened.tp,
        [
            roll_forward_msg(peer_a, conn_a, &opened.handler, &headers[1]),
            roll_forward_msg(peer_b, conn_b, &handler_b, &headers[1]),
        ],
    );
    drive(&mut opened.running, prep.rt.handle()).assert_sleeping();

    let state = opened.running.get_state(&opened.tp).expect("both peers were processed");
    assert_eq!(owed(state, conn_a), Some(1));
    assert_eq!(owed(state, conn_b), Some(0));
    assert_eq!(current_point(state, conn_a), Some(headers[1].point()));
    assert_eq!(current_point(state, conn_b), Some(headers[1].point()));
    let admission = opened.running.trace_buffer().lock().hydrate_without_timestamps();
    assert_trace_contains(
        &opened.running,
        &[tm_try_send("tp-1", "handler", RequestNext), tm_try_send("tp-1", "", RequestNext)],
    );
    assert_try_send_resumes(&admission, &[TrySend::Full, TrySend::Queued]);
}

/// `Gone` drops the miss immediately. `Terminated` still drops the session.
#[test]
fn gone_handler_is_purged_by_terminated() {
    let headers = linked_headers(2);
    let ready = ready_peer(&headers);
    let mut opened =
        open_fanout(ready.rt.handle(), ready.state, build_store(slice::from_ref(&headers[0])), HandlerHold::Hour);
    let gone = StageRef::<InitiatorMessage>::named_for_tests("gone-handler");
    opened.running.enqueue_msg(&opened.tp, [roll_forward_msg(ready.peer, ready.conn_id, &gone, &headers[1])]);
    drive(&mut opened.running, ready.rt.handle()).assert_idle();
    let state = opened.running.get_state(&opened.tp).expect("idle after a gone send");
    assert_eq!(owed(state, ready.conn_id), Some(0));
    assert!(!state.request_retry_armed);
    let admission = opened.running.trace_buffer().lock().hydrate_without_timestamps();
    assert_trace_contains(&opened.running, &[tm_try_send("tp-1", "gone-handler", RequestNext)]);
    assert_try_send_resumes(&admission, &[TrySend::Gone]);

    opened.running.enqueue_msg(
        &opened.tp,
        [TrackPeersMsg::FromUpstream(ChainSyncInitiatorMsg {
            peer: ready.peer,
            conn_id: ready.conn_id,
            handler: gone,
            msg: chainsync::InitiatorResult::Terminated,
        })],
    );
    drive(&mut opened.running, ready.rt.handle());
    let state = opened.running.get_state(&opened.tp).expect("idle after terminate");
    assert_eq!(owed(state, ready.conn_id), None);
    assert!(!state.request_retry_armed);
    assert!(state.upstream.is_empty());
}

/// `Gone` leaves no owed count and no armed retry. A later header is not offered to that
/// handler. Another session that still owes keeps the one retry slot. A `Gone` on the retry
/// of a counted slot drops that count, sends nothing further, and clears the slot when
/// nobody else owes one.
#[test]
fn gone_handler_drops_owed_and_is_not_asked_again() {
    let headers = linked_headers(4);
    let prep = test_prep();
    let peer_a = Peer::for_test(3001);
    let peer_b = Peer::for_test(3002);
    let mut ids = ConnectionId::initial();
    let conn_a = ids.get_and_increment();
    let conn_b = ids.get_and_increment();
    let mut state = prep.state;
    state.insert_peer(peer_a, conn_a, headers[0].point(), headers[0].point());
    state.insert_peer(peer_b, conn_b, headers[0].point(), headers[0].point());

    let mut opened = open_fanout(prep.rt.handle(), state, build_store(slice::from_ref(&headers[0])), HandlerHold::Hour);
    park_handler_full(&mut opened.running, &opened.handler);
    opened.running.enqueue_msg(&opened.tp, [roll_forward_msg(peer_a, conn_a, &opened.handler, &headers[1])]);
    drive(&mut opened.running, prep.rt.handle()).assert_sleeping();
    assert_eq!(owed(opened.running.get_state(&opened.tp).expect("idle"), conn_a), Some(1));

    let gone = StageRef::<InitiatorMessage>::named_for_tests("gone-handler");
    opened.running.trace_buffer().lock().clear();
    opened.running.enqueue_msg(&opened.tp, [roll_forward_msg(peer_b, conn_b, &gone, &headers[1])]);
    drive(&mut opened.running, prep.rt.handle()).assert_sleeping();
    let state = opened.running.get_state(&opened.tp).expect("peer b is gone");
    assert_eq!(owed(state, conn_b), Some(0));
    assert_eq!(owed(state, conn_a), Some(1));
    assert!(state.request_retry_armed);
    let admission = opened.running.trace_buffer().lock().hydrate_without_timestamps();
    assert_try_send_resumes(&admission, &[TrySend::Gone]);

    opened.running.trace_buffer().lock().clear();
    opened.running.enqueue_msg(&opened.tp, [roll_forward_msg(peer_b, conn_b, &gone, &headers[2])]);
    drive(&mut opened.running, prep.rt.handle()).assert_sleeping();
    let state = opened.running.get_state(&opened.tp).expect("peer b was not asked again");
    assert_eq!(owed(state, conn_b), Some(0));
    assert!(state.request_retry_armed);
    assert_eq!(current_point(state, conn_b), Some(headers[2].point()));
    let admission = opened.running.trace_buffer().lock().hydrate_without_timestamps();
    assert_try_send_resumes(&admission, &[]);

    opened.running.trace_buffer().lock().clear();
    opened.running.enqueue_msg(&opened.tp, [roll_forward_msg(peer_a, conn_a, &gone, &headers[2])]);
    drive(&mut opened.running, prep.rt.handle()).assert_sleeping();
    let state = opened.running.get_state(&opened.tp).expect("counted slot dropped");
    assert_eq!(owed(state, conn_a), Some(0));
    assert!(!state.request_retry_armed);
    assert_eq!(current_point(state, conn_a), Some(headers[2].point()));
    let admission = opened.running.trace_buffer().lock().hydrate_without_timestamps();
    assert_try_send_resumes(&admission, &[TrySend::Gone]);
    assert_trace_does_not_contain(&opened.running, &[tm_retry_timeout()]);
}

/// A handler that accepts every `RequestNext` does not arm the retry timeout.
#[test]
fn no_retry_timeout_when_nothing_is_owed() {
    let prep = test_prep();
    let peer = Peer::for_test(3001);
    let parent = &prep.headers[0];
    let header = &prep.headers[1];
    let mut state = prep.state.clone();
    state.insert_peer(peer, prep.conn_id, parent.point(), parent.point());
    let msg = roll_forward_msg(peer, prep.conn_id, &prep.handler, header);
    let (running, _guards, _logs) =
        setup(&prep.rt_handle(), state, msg, build_store_with_nonces(slice::from_ref(header)));
    assert_trace_contains(
        &running,
        &[tm_state::<TrackPeers>(
            "tp-1",
            |s| !s.request_retry_armed && s.recheck_timer.is_none() && owed(s, prep.conn_id) == Some(0),
            "nothing owed and no retry timeout",
        )],
    );
    assert_trace_does_not_contain(&running, &[tm_retry_timeout()]);
}

/// After the handler drains, one retry admits every owed `RequestNext` that now fits.
#[test]
fn handler_drain_refills_the_window_to_pipeline_depth() {
    let depth = u64::from(PIPELINE_DEPTH);
    let headers = linked_headers(depth + 1);
    let ready = ready_peer(&headers);
    let mut opened = open_fanout(
        ready.rt.handle(),
        ready.state,
        build_store(slice::from_ref(&headers[0])),
        HandlerHold::FirstMillis,
    );
    let parked = park_handler_full(&mut opened.running, &opened.handler);
    for header in headers.iter().skip(1) {
        opened.running.enqueue_msg(&opened.tp, [roll_forward_msg(ready.peer, ready.conn_id, &opened.handler, header)]);
        drive(&mut opened.running, ready.rt.handle()).assert_sleeping();
    }
    assert_eq!(owed(opened.running.get_state(&opened.tp).expect("idle"), ready.conn_id), Some(PIPELINE_DEPTH));

    opened.running.run(Run::until(parked)).assert_sleeping();
    opened.running.trace_buffer().lock().clear();
    let retry_at = opened.running.run(Run::default()).assert_sleeping();
    opened.running.run(Run::until(retry_at));

    let state = opened.running.get_state(&opened.tp).expect("window refilled");
    assert_eq!(owed(state, ready.conn_id), Some(0));
    assert!(!state.request_retry_armed);
    let queued: Vec<_> = (0..PIPELINE_DEPTH).map(|_| tm_try_send("tp-1", "handler", RequestNext)).collect();
    let admission = opened.running.trace_buffer().lock().hydrate_without_timestamps();
    assert_trace_match_filter(&opened.running, &queued, &[not_try_send()]);
    let outcomes: Vec<_> = (0..PIPELINE_DEPTH).map(|_| TrySend::Queued).collect();
    assert_try_send_resumes(&admission, &outcomes);
}

/// A mailbox with `k` free slots takes `k` owed requests in one retry. The next is `Full`,
/// so the loop stops and the single retry slot is armed again for what is still owed.
#[test]
fn retry_fills_free_slots_then_stops_and_rearms() {
    const FREE: u8 = 3;
    const OWED: u8 = 4;
    let headers = linked_headers(u64::from(OWED) + 1);
    let ready = ready_peer(&headers);
    let mut opened =
        open_fanout_quick_then_hour(ready.rt.handle(), ready.state, build_store(slice::from_ref(&headers[0])), FREE);
    park_handler_full(&mut opened.running, &opened.handler);
    for header in headers.iter().skip(1) {
        opened.running.enqueue_msg(&opened.tp, [roll_forward_msg(ready.peer, ready.conn_id, &opened.handler, header)]);
        drive(&mut opened.running, ready.rt.handle()).assert_sleeping();
    }
    assert_eq!(owed(opened.running.get_state(&opened.tp).expect("idle"), ready.conn_id), Some(OWED));

    for _ in 0..FREE {
        let wake = opened.running.run(Run::default()).assert_sleeping();
        opened.running.run(Run::until(wake)).assert_sleeping();
    }
    opened.running.trace_buffer().lock().clear();
    let retry_at = opened.running.run(Run::default()).assert_sleeping();
    opened.running.run(Run::until(retry_at));

    let state = opened.running.get_state(&opened.tp).expect("retry stopped on a full mailbox");
    assert_eq!(owed(state, ready.conn_id), Some(OWED - FREE));
    assert!(state.request_retry_armed);
    let mut expected: Vec<_> = (0..FREE).map(|_| tm_try_send("tp-1", "handler", RequestNext)).collect();
    expected.push(tm_try_send("tp-1", "handler", RequestNext));
    expected.push(tm_retry_timeout());
    let admission = opened.running.trace_buffer().lock().hydrate_without_timestamps();
    assert_trace_match_filter(&opened.running, &expected, &[not_admission()]);
    let mut outcomes = vec![TrySend::Queued; usize::from(FREE)];
    outcomes.push(TrySend::Full);
    assert_try_send_resumes(&admission, &outcomes);
}
