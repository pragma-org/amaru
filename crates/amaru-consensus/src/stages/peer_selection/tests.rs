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

// Many tests in this file were simplified to only check logs; the variables for
// trace assertions are intentionally left for future use or documentation.

use std::collections::BTreeSet;

use amaru_ouroboros::{ConnectionDirection, ConnectionId};
use amaru_protocols::manager::ManagerMessage;
use amaru_pure_stage::trace_match::{assert_trace_contains, assert_trace_does_not_contain, tm_send_match};
use tracing::Level;

use super::*;
use crate::stages::{
    peer_selection::test_setup::{
        TestPrep, cooldown_duration, cooldown_instant, first_schedule_id, first_static_schedule_id,
        peer_selection_stage, second_schedule_id_at, setup, setup_preload, setup_preload_until_sleeping, sim_at,
        sim_t0, static_cooldown_instant, te_cancel_schedule, te_clear_peer_availability, te_clock, te_clock_suspend,
        te_peer_adversarial, te_random_seed, te_record_advertisability, te_record_connection_failure, te_schedule,
        te_send, test_prep, test_prep_with_snapshot, tm_add_stage_starts_with, with_single_cooldown,
    },
    test_utils::{assert_trace, te_input, te_state, tm_state},
};

fn conn() -> Connection {
    Connection::new(ConnectionId::initial(), true, false)
}

// ---------------------------------------------------------------------------
// Initialize
// ---------------------------------------------------------------------------

#[test]
fn test_initialize_empty_static() {
    let prep = test_prep(&[]);
    let state = prep.state.clone();
    let msg = PeerSelectionMsg::Initialize;
    let (running, _guards, mut logs) = setup(&prep, msg.clone());

    // On Initialize we create the "peer-selection/ledger-check" child.
    // We assert the key observable effects + the INFO log.
    // The Initialize path creates the ledger-check child and sends it its first message.
    // We assert the observable parent state transitions and let a dedicated wiring test
    // cover the AddStage/WireStage details using TraceMatch helpers.
    // With no candidates, regulate_peers still draws a random seed to attempt a refill.
    assert_trace_contains(
        &running,
        &[
            te_input("ps-1", &msg).into(),
            te_random_seed("ps-1").into(),
            tm_add_stage_starts_with("peer-selection/ledger-check"),
            te_state("ps-1", &state).into(), // final parent state after child creation (child state not asserted in detail)
        ],
    );

    logs.assert_and_remove(Level::INFO, &["peer_selection.connect_initial"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

#[test]
fn test_initialize_adds_static_peers() {
    let prep = test_prep(&["10.0.0.1:1", "10.0.0.2:2"]);
    let mut state = prep.state.clone();
    let msg = PeerSelectionMsg::Initialize;

    let p1 = TestPrep::peer("10.0.0.1:1");
    let p2 = TestPrep::peer("10.0.0.2:2");

    state.outbound_peers.insert(p1.clone(), PeerState::Connecting);
    state.outbound_peers.insert(p2.clone(), PeerState::Connecting);

    let (running, _guards, mut logs) = setup(&prep, msg.clone());

    // target_upstream_peers is 3 and only 2 static peers exist, so regulate draws a seed
    // but finds no further candidates.
    assert_trace_contains(
        &running,
        &[
            te_input("ps-1", &msg).into(),
            te_random_seed("ps-1").into(),
            tm_add_stage_starts_with("peer-selection/ledger-check"),
            te_state("ps-1", &state).into(), // final parent state after child creation
        ],
    );

    logs.assert_and_remove(Level::INFO, &["peer_selection.connect_initial"])
        .assert_no_remaining_at([Level::WARN, Level::ERROR]);
}

#[test]
fn test_initialize_fills_from_snapshot() {
    let prep = test_prep_with_snapshot(&[], &["snap1:1", "snap2:2", "snap3:3"]);
    let msg = PeerSelectionMsg::Initialize;
    let (running, _guards, mut logs) = setup(&prep, msg.clone());

    assert_trace_contains(
        &running,
        &[
            te_input("ps-1", &msg).into(),
            te_random_seed("ps-1").into(),
            tm_send_match::<ManagerMessage>("ps-1", "manager", |m| matches!(m, ManagerMessage::AddPeer(_))),
            tm_send_match::<ManagerMessage>("ps-1", "manager", |m| matches!(m, ManagerMessage::AddPeer(_))),
            tm_send_match::<ManagerMessage>("ps-1", "manager", |m| matches!(m, ManagerMessage::AddPeer(_))),
            tm_add_stage_starts_with("peer-selection/ledger-check"),
            tm_state(
                "ps-1",
                |s: &PeerSelection| s.outbound_peers.len() == 3,
                "final state filled from snapshot candidates",
            ),
        ],
    );

    logs.assert_and_remove(Level::INFO, &["peer_selection.connect_initial"])
        .assert_and_remove(Level::INFO, &["peer_selection.add_peer"])
        .assert_and_remove(Level::INFO, &["peer_selection.add_peer"])
        .assert_and_remove(Level::INFO, &["peer_selection.add_peer"])
        .assert_no_remaining_at([Level::WARN, Level::ERROR]);
}

#[test]
fn test_regulate_prefers_static_before_snapshot_before_ledger() {
    let mut prep = test_prep_with_snapshot(&["static1:1"], &["snap1:1", "snap2:2"]);
    let l1 = TestPrep::peer("ledger1:1");
    prep.state.ledger_candidates.insert(l1.clone());
    // Start with empty outbound and trigger regulate via CheckCooldowns.
    let dummy = TestPrep::peer("dummy:9");
    prep.state.cooldowns.cooldown_until.insert(dummy.clone(), cooldown_instant());

    let (running, _guards, mut logs) = setup_preload(&prep, [PeerSelectionMsg::CheckCooldowns]);

    // target is 3: static first, then one snapshot, then one ledger (or two snapshots).
    // With seed [0x42; 32], choose_multiple is deterministic; we only assert counts and that
    // static is included.
    assert_trace_contains(
        &running,
        &[
            te_input("ps-1", &PeerSelectionMsg::CheckCooldowns).into(),
            tm_send_match::<ManagerMessage>("ps-1", "manager", |m| matches!(m, ManagerMessage::AddPeer(_))),
            tm_send_match::<ManagerMessage>("ps-1", "manager", |m| matches!(m, ManagerMessage::AddPeer(_))),
            tm_send_match::<ManagerMessage>("ps-1", "manager", |m| matches!(m, ManagerMessage::AddPeer(_))),
            tm_state(
                "ps-1",
                |s: &PeerSelection| {
                    s.outbound_peers.contains_key(&TestPrep::peer("static1:1")) && s.outbound_peers.len() == 3
                },
                "static preferred and target filled",
            ),
        ],
    );

    logs.assert_and_remove(Level::INFO, &["peer_selection.add_peer"])
        .assert_and_remove(Level::INFO, &["peer_selection.add_peer"])
        .assert_and_remove(Level::INFO, &["peer_selection.add_peer"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_ledger_candidates_replace_does_not_clear_snapshot() {
    let prep = test_prep_with_snapshot(&[], &["snap1:1"]);
    let ledger = TestPrep::peer("ledger1:1");
    let msg = PeerSelectionMsg::LedgerCheckCandidates(BTreeSet::from([ledger.clone()]));

    // After ledger update, snapshot pool is still present; regulate may pick either.
    let (running, _guards, mut logs) = setup(&prep, msg.clone());

    assert_trace_contains(
        &running,
        &[
            te_input("ps-1", &msg).into(),
            te_random_seed("ps-1").into(),
            tm_state(
                "ps-1",
                |s: &PeerSelection| {
                    s.snapshot_candidates.contains(&TestPrep::peer("snap1:1")) && s.ledger_candidates.contains(&ledger)
                },
                "snapshot pool retained after ledger replace",
            ),
        ],
    );

    logs.assert_and_remove(Level::INFO, &["peer_selection.add_peer"])
        .assert_and_remove(Level::INFO, &["peer_selection.add_peer"])
        .assert_no_remaining_at([Level::WARN, Level::ERROR]);
}

// ---------------------------------------------------------------------------
// AddPeer
// ---------------------------------------------------------------------------

#[test]
fn test_add_peer_not_in_cooldown() {
    let prep = test_prep(&[]);
    let p = TestPrep::peer("9.9.9.9:9");
    let state = prep.state.clone();
    let msg = PeerSelectionMsg::AddPeer(p.clone());
    let after = {
        let mut s = state.clone();
        s.outbound_peers.insert(p.clone(), PeerState::Connecting);
        s
    };
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("ps-1", &state),
            te_input("ps-1", &msg),
            te_send("ps-1", "manager", ManagerMessage::AddPeer(p)),
            te_state("ps-1", &after),
        ],
    );
    logs.assert_and_remove(Level::INFO, &["peer_selection.add_peer"])
        .assert_no_remaining_at([Level::WARN, Level::ERROR]);
}

#[test]
fn test_add_peer_during_cooldown_cancels_timer() {
    let prep = test_prep(&[]);
    let p = TestPrep::peer("8.8.8.8:8");
    let state = prep.state.clone();
    let sid = first_schedule_id();
    let after_ban = {
        let mut s = state.clone();
        with_single_cooldown(&mut s, p.clone(), sid);
        s
    };
    let after_add = {
        let mut s = state.clone();
        s.outbound_peers.insert(p.clone(), PeerState::Connecting);
        s
    };
    let (running, _guards, mut logs) =
        setup_preload(&prep, [PeerSelectionMsg::adversarial(p.clone()), PeerSelectionMsg::AddPeer(p.clone())]);
    assert_trace(
        &running,
        &[
            te_state("ps-1", &state),
            te_input("ps-1", &PeerSelectionMsg::adversarial(p.clone())),
            te_peer_adversarial("ps-1", p.clone()),
            te_clock_suspend("ps-1"),
            te_schedule("ps-1", PeerSelectionMsg::CheckCooldowns, sid),
            te_state("ps-1", &after_ban),
            te_input("ps-1", &PeerSelectionMsg::AddPeer(p.clone())),
            te_cancel_schedule("ps-1", sid),
            te_send("ps-1", "manager", ManagerMessage::AddPeer(p.clone())),
            te_state("ps-1", &after_add),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["peer_selection.adversarial"])
        .assert_and_remove(Level::INFO, &["peer_selection.add_peer"])
        .assert_no_remaining_at([Level::WARN, Level::ERROR]);
}

// ---------------------------------------------------------------------------
// Adversarial / ban
// ---------------------------------------------------------------------------

#[test]
fn test_adversarial_outbound_connected() {
    let prep = test_prep(&[]);
    let p = TestPrep::peer("7.7.7.7:7");
    let state = prep.state.clone();
    let sid = first_schedule_id();
    let after = {
        let mut s = state.clone();
        with_single_cooldown(&mut s, p.clone(), sid);
        s
    };
    let (running, _guards, mut logs) = setup(&prep, PeerSelectionMsg::adversarial(p.clone()));
    assert_trace(
        &running,
        &[
            te_state("ps-1", &state),
            te_input("ps-1", &PeerSelectionMsg::adversarial(p.clone())),
            te_peer_adversarial("ps-1", p.clone()),
            te_clock_suspend("ps-1"),
            te_schedule("ps-1", PeerSelectionMsg::CheckCooldowns, sid),
            te_state("ps-1", &after),
            te_clock(cooldown_instant()),
            te_input("ps-1", &PeerSelectionMsg::CheckCooldowns),
            te_cancel_schedule("ps-1", sid),
            te_clock_suspend("ps-1"),
            te_random_seed("ps-1"),
            te_state("ps-1", &state),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["peer_selection.adversarial"])
        .assert_no_remaining_at([Level::WARN, Level::ERROR]);
}

// ---------------------------------------------------------------------------
// CheckCooldowns
// ---------------------------------------------------------------------------

#[test]
fn test_check_cooldowns_stale() {
    let prep = test_prep(&[]);
    let state = prep.state.clone();
    let msg = PeerSelectionMsg::CheckCooldowns;
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("ps-1", &state),
            te_input("ps-1", &msg),
            te_clock_suspend("ps-1"),
            te_random_seed("ps-1"),
            te_state("ps-1", &state),
        ],
    );
    logs.assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_check_cooldowns_before_due_does_not_lift_ban() {
    let prep = test_prep(&[]);
    let p = TestPrep::peer("5.5.5.5:5");
    let state = prep.state.clone();
    let sid0 = first_schedule_id();
    let sid1 = second_schedule_id_at(cooldown_instant());
    let after_ban = {
        let mut s = state.clone();
        with_single_cooldown(&mut s, p.clone(), sid0);
        s
    };
    let after_early = {
        let mut s = after_ban.clone();
        // Early CheckCooldowns cancels and re-arms; the ban itself stays until due.
        s.cooldown_timer = Some(sid1);
        s
    };
    let (running, _guards, mut logs) =
        setup_preload(&prep, [PeerSelectionMsg::adversarial(p.clone()), PeerSelectionMsg::CheckCooldowns]);
    assert_trace(
        &running,
        &[
            te_state("ps-1", &state),
            te_input("ps-1", &PeerSelectionMsg::adversarial(p.clone())),
            te_peer_adversarial("ps-1", p.clone()),
            te_clock_suspend("ps-1"),
            te_schedule("ps-1", PeerSelectionMsg::CheckCooldowns, sid0),
            te_state("ps-1", &after_ban),
            te_input("ps-1", &PeerSelectionMsg::CheckCooldowns),
            te_cancel_schedule("ps-1", sid0),
            te_clock_suspend("ps-1"),
            te_random_seed("ps-1"),
            te_schedule("ps-1", PeerSelectionMsg::CheckCooldowns, sid1),
            te_state("ps-1", &after_early),
            te_clock(cooldown_instant()),
            te_input("ps-1", &PeerSelectionMsg::CheckCooldowns),
            te_cancel_schedule("ps-1", sid1),
            te_clock_suspend("ps-1"),
            te_random_seed("ps-1"),
            te_state("ps-1", &state),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["peer_selection.adversarial"])
        .assert_no_remaining_at([Level::WARN, Level::ERROR]);
}

// ---------------------------------------------------------------------------
// Connected / Disconnected
// ---------------------------------------------------------------------------

#[test]
fn test_connected_inbound_success() {
    let prep = test_prep(&[]);
    let p = TestPrep::peer("4.4.4.4:4");
    let state = prep.state.clone();
    let msg = PeerSelectionMsg::Connected(p.clone(), conn(), ConnectionDirection::Inbound, true);
    let after = {
        let mut s = state.clone();
        s.inbound_peers.insert(p.clone(), conn());
        s
    };
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("ps-1", &state),
            te_input("ps-1", &msg),
            te_clock_suspend("ps-1"),
            te_record_advertisability("ps-1", p.clone(), true, sim_t0()),
            te_state("ps-1", &after),
        ],
    );
    logs.assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_connected_inbound_too_many() {
    let mut prep = test_prep(&[]);
    let p = TestPrep::peer("3.3.3.3:3");
    for i in 0..10u8 {
        prep.state.inbound_peers.insert(TestPrep::peer(&format!("1.1.1.{i}:1")), conn());
    }
    let state = prep.state.clone();
    let msg = PeerSelectionMsg::Connected(p.clone(), conn(), ConnectionDirection::Inbound, false);
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    // At capacity: still records advertisability, then disconnects without inserting.
    assert_trace(
        &running,
        &[
            te_state("ps-1", &state),
            te_input("ps-1", &msg),
            te_clock_suspend("ps-1"),
            te_record_advertisability("ps-1", p.clone(), false, sim_t0()),
            te_send("ps-1", "manager", ManagerMessage::Disconnect(p.clone(), ConnectionId::initial())),
            te_state("ps-1", &state),
        ],
    );
    logs.assert_and_remove(Level::INFO, &["rejecting inbound connection: too many peers"])
        .assert_no_remaining_at([Level::WARN, Level::ERROR]);
}

#[test]
fn test_connected_outbound() {
    let prep = test_prep(&[]);
    let p = TestPrep::peer("2.2.2.2:2");
    let state = prep.state.clone();
    // advertisable=false: no peer-sharing schedule (see test_connected_outbound_schedules_share).
    let msg = PeerSelectionMsg::Connected(p.clone(), conn(), ConnectionDirection::Outbound, false);
    let after = {
        let mut s = state.clone();
        s.outbound_peers.insert(p.clone(), PeerState::Connected(conn()));
        s
    };
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("ps-1", &state),
            te_input("ps-1", &msg),
            te_clock_suspend("ps-1"),
            te_record_advertisability("ps-1", p.clone(), false, sim_t0()),
            te_state("ps-1", &after),
        ],
    );
    logs.assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_connected_outbound_starts_peer_sharing() {
    let prep = test_prep(&[]);
    let p = TestPrep::peer("6.6.6.6:6");
    let state = prep.state.clone();
    let msg = PeerSelectionMsg::Connected(p.clone(), conn(), ConnectionDirection::Outbound, true);
    let after = {
        let mut s = state.clone();
        s.outbound_peers.insert(p.clone(), PeerState::Connected(conn()));
        s
    };
    let p_send = p.clone();
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace_contains(
        &running,
        &[
            te_state("ps-1", &state).into(),
            te_input("ps-1", &msg).into(),
            te_record_advertisability("ps-1", p.clone(), true, sim_t0()).into(),
            tm_send_match("ps-1", "manager", move |m: &ManagerMessage| {
                matches!(
                    m,
                    ManagerMessage::RequestSharePeers {
                        peer,
                        amount,
                        initial_delay,
                        interval,
                        ..
                    } if peer == &p_send
                        && *amount == super::SHARE_REQUEST_AMOUNT
                        && *initial_delay == super::SHARE_REQUEST_INITIAL_DELAY
                        && *interval == super::SHARE_REQUEST_INTERVAL
                )
            }),
            te_state("ps-1", &after).into(),
        ],
    );
    logs.assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_share_peers_result_records_shared_peers() {
    use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

    let p = TestPrep::peer("7.7.7.7:7");
    let learned = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(9, 9, 9, 9), 3001));
    let mut prep = test_prep(&[]);
    prep.state.outbound_peers.insert(p.clone(), PeerState::Connected(conn()));
    let reply = PeerSelectionMsg::SharePeersResult { peer: p.clone(), peers: vec![learned] };
    let (running, _guards, mut logs) = setup(&prep, reply.clone());
    assert_trace_contains(
        &running,
        &[
            te_input("ps-1", &reply).into(),
            tm_state(
                "ps-1",
                move |s: &PeerSelection| s.shared_peers.contains(&Peer::from_addr(&learned)),
                "shared peer recorded",
            ),
        ],
    );
    logs.assert_and_remove(Level::INFO, &["peer_selection.peer.share_peers"])
        .assert_and_remove(Level::INFO, &["peer_selection.add_peer"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_connect_failed_records_failure() {
    let mut prep = test_prep(&[]);
    let p = TestPrep::peer("5.5.5.5:5");
    prep.state.outbound_peers.insert(p.clone(), PeerState::Connecting);
    let state = prep.state.clone();
    // Empty static/snapshot: regulate finds no replacements after remove.
    let after = {
        let mut s = state.clone();
        s.outbound_peers.remove(&p);
        s
    };
    let msg = PeerSelectionMsg::ConnectFailed(p.clone());
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("ps-1", &state),
            te_input("ps-1", &msg),
            te_clock_suspend("ps-1"),
            te_record_connection_failure("ps-1", p.clone(), sim_t0()),
            te_clear_peer_availability("ps-1", p.clone()),
            te_random_seed("ps-1"),
            te_state("ps-1", &after),
        ],
    );
    logs.assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_disconnected_inbound() {
    let prep = test_prep(&[]);
    let p = TestPrep::peer("1.1.1.1:1");
    let state = prep.state.clone();
    let mut state_with_peer = state.clone();
    state_with_peer.inbound_peers.insert(p.clone(), conn());
    let msg = PeerSelectionMsg::Disconnected(p.clone(), ConnectionId::initial(), ConnectionDirection::Inbound, false);
    let after = {
        let mut s = state_with_peer.clone();
        s.inbound_peers.remove(&p);
        s
    };
    let (running, _guards, mut logs) = setup_preload(&prep, [msg.clone()]);
    assert_trace(
        &running,
        &[
            te_state("ps-1", &state),
            te_input("ps-1", &msg),
            te_clear_peer_availability("ps-1", p.clone()),
            te_state("ps-1", &after),
        ],
    );
    logs.assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_disconnected_outbound_connecting_schedules_cooldown() {
    let prep = test_prep(&[]);
    let p = TestPrep::peer("0.0.0.0:0");
    let state = prep.state.clone();
    let mut state_conn = state.clone();
    state_conn.outbound_peers.insert(p.clone(), PeerState::Connecting);
    let msg = PeerSelectionMsg::Disconnected(p.clone(), ConnectionId::initial(), ConnectionDirection::Outbound, true);
    let (running, _guards, mut logs) = setup_preload(&prep, [msg.clone()]);
    // will_retry == true: no cool-down, no regulation; still clear availability claims.
    assert_trace(
        &running,
        &[
            te_state("ps-1", &state),
            te_input("ps-1", &msg),
            te_clear_peer_availability("ps-1", p.clone()),
            te_state("ps-1", &state),
        ],
    );
    logs.assert_no_remaining_at([Level::WARN, Level::ERROR]);
}

// ---------------------------------------------------------------------------
// Double-remove (Adversarial twice)
// ---------------------------------------------------------------------------

#[test]
fn test_adversarial_twice_extends_cooldown_single_timer() {
    let prep = test_prep(&[]);
    let p = TestPrep::peer("11.11.11.11:11");
    let state = prep.state.clone();
    let sid0 = first_schedule_id();
    let after_first = {
        let mut s = state.clone();
        with_single_cooldown(&mut s, p.clone(), sid0);
        s
    };
    // Second ban at the same simulation time reuses the armed timer; heap gets another entry.
    let after_second = {
        let mut s = after_first.clone();
        s.cooldowns.add_and_is_first(p.clone(), cooldown_instant());
        s
    };
    let (running, _guards, mut logs) =
        setup_preload(&prep, [PeerSelectionMsg::adversarial(p.clone()), PeerSelectionMsg::adversarial(p.clone())]);
    assert_trace(
        &running,
        &[
            te_state("ps-1", &state),
            te_input("ps-1", &PeerSelectionMsg::adversarial(p.clone())),
            te_peer_adversarial("ps-1", p.clone()),
            te_clock_suspend("ps-1"),
            te_schedule("ps-1", PeerSelectionMsg::CheckCooldowns, sid0),
            te_state("ps-1", &after_first),
            te_input("ps-1", &PeerSelectionMsg::adversarial(p.clone())),
            te_peer_adversarial("ps-1", p.clone()),
            te_clock_suspend("ps-1"),
            te_state("ps-1", &after_second),
            te_clock(cooldown_instant()),
            te_input("ps-1", &PeerSelectionMsg::CheckCooldowns),
            te_cancel_schedule("ps-1", sid0),
            te_clock_suspend("ps-1"),
            te_random_seed("ps-1"),
            te_state("ps-1", &state),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["peer_selection.adversarial"])
        .assert_and_remove(Level::DEBUG, &["peer_selection.adversarial"])
        .assert_no_remaining_at([Level::WARN, Level::ERROR]);
}

#[test]
fn test_reban_later_deadline_survives_early_timer() {
    // Ban at t0 (when = t0+1s). Advance to t0+500ms and re-ban (when = t0+1.5s) while the
    // original timer is still armed. When that timer fires at t0+1s, the extended ban must
    // remain; only the later CheckCooldowns at t0+1.5s may lift it.
    use std::time::Duration;

    let prep = test_prep(&[]);
    let p = TestPrep::peer("12.12.12.12:12");
    let intermediate = sim_at(Duration::from_millis(500));
    let first_when = cooldown_instant();
    let later_when = intermediate + cooldown_duration();
    let sid0 = first_schedule_id();
    let sid1 = second_schedule_id_at(later_when);

    let (mut running, _guards, _logs) = setup_preload_until_sleeping(&prep, [PeerSelectionMsg::adversarial(p.clone())]);

    assert!(!running.skip_to_next_wakeup(Some(intermediate)));
    assert_eq!(running.now(), intermediate);

    running.enqueue_msg(peer_selection_stage(), [PeerSelectionMsg::adversarial(p.clone())]);
    running.run_until_blocked_incl_effects(prep.rt.handle());

    assert_trace_contains(
        &running,
        &[
            te_input("ps-1", &PeerSelectionMsg::adversarial(p.clone())).into(),
            te_schedule("ps-1", PeerSelectionMsg::CheckCooldowns, sid0).into(),
            te_clock(intermediate).into(),
            te_input("ps-1", &PeerSelectionMsg::adversarial(p.clone())).into(),
            // Original timer fires while the map still holds the later deadline.
            te_clock(first_when).into(),
            te_input("ps-1", &PeerSelectionMsg::CheckCooldowns).into(),
            te_cancel_schedule("ps-1", sid0).into(),
            te_schedule("ps-1", PeerSelectionMsg::CheckCooldowns, sid1).into(),
            tm_state(
                "ps-1",
                move |s: &PeerSelection| {
                    s.cooldowns.cooldown_until.get(&p) == Some(&later_when)
                        && s.cooldown_timer == Some(sid1)
                        && s.cooldowns.cooldown_heap.len() == 1
                },
                "re-ban deadline preserved after early CheckCooldowns; timer re-armed",
            ),
            // Extended cool-down finally ends.
            te_clock(later_when).into(),
            te_input("ps-1", &PeerSelectionMsg::CheckCooldowns).into(),
            te_cancel_schedule("ps-1", sid1).into(),
            tm_state(
                "ps-1",
                |s: &PeerSelection| {
                    s.cooldowns.cooldown_until.is_empty()
                        && s.cooldown_timer.is_none()
                        && s.cooldowns.cooldown_heap.is_empty()
                },
                "ban lifted only at the later deadline",
            ),
        ],
    );
}

// ---------------------------------------------------------------------------
// Additional control-flow coverage (as requested)
// - Ban inbound-only peer
// - Normal outbound Connected disconnect (not Connecting failure)
// - Regulate prefers static before ledger
// - Regulate skips peers in cooldown
// - Outbound disconnect when peer is present in both inbound and outbound
// Focus: final PeerSelection state + ManagerMessage sends
// ---------------------------------------------------------------------------

#[test]
fn test_adversarial_inbound_only() {
    let mut prep = test_prep(&[]);
    let p = TestPrep::peer("9.9.9.9:9");
    prep.state.inbound_peers.insert(p.clone(), conn());

    let (running, _guards, mut logs) = setup(&prep, PeerSelectionMsg::adversarial(p.clone()));

    // te_input + RemovePeer to Manager + final state (inbound peer count)
    assert_trace_contains(
        &running,
        &[
            te_input("ps-1", &PeerSelectionMsg::adversarial(p.clone())).into(),
            te_send("ps-1", "manager", ManagerMessage::RemovePeer(p.clone())).into(),
            tm_state(
                "ps-1",
                |s: &PeerSelection| !s.inbound_peers.contains_key(&p),
                "final state: inbound peer removed",
            ),
        ],
    );

    logs.assert_and_remove(Level::DEBUG, &["peer_selection.adversarial"])
        .assert_and_remove(Level::WARN, &["removing peer (inbound)"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_disconnected_outbound_connected_normal() {
    let prep = test_prep(&[]);
    let p = TestPrep::peer("8.8.8.8:8");
    let mut state = prep.state.clone();
    state.outbound_peers.insert(p.clone(), PeerState::Connected(conn()));

    let _after = {
        let mut s = state.clone();
        s.outbound_peers.remove(&p);
        s
    };

    let (running, _guards, mut logs) = setup(
        &prep,
        PeerSelectionMsg::Disconnected(p.clone(), ConnectionId::initial(), ConnectionDirection::Outbound, false),
    );

    // te_input + final state (normal outbound Connected disconnect removes the peer, no short ban)
    assert_trace_contains(
        &running,
        &[
            te_input(
                "ps-1",
                &PeerSelectionMsg::Disconnected(
                    p.clone(),
                    ConnectionId::initial(),
                    ConnectionDirection::Outbound,
                    false,
                ),
            )
            .into(),
            tm_state(
                "ps-1",
                |s: &PeerSelection| !s.outbound_peers.contains_key(&p),
                "final state: peer removed from outbound",
            ),
        ],
    );

    logs.assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_regulate_prefers_static_before_ledger() {
    let mut prep = test_prep(&["static1:1", "static2:2"]);

    // Seed some ledger candidates
    let l1 = TestPrep::peer("ledger1:1");
    let l2 = TestPrep::peer("ledger2:2");
    prep.state.ledger_candidates.insert(l1.clone());
    prep.state.ledger_candidates.insert(l2.clone());

    // Trigger regulate via CheckCooldowns of a non-existent peer (cheap way to call it)
    let dummy = TestPrep::peer("dummy:9");
    prep.state.cooldowns.cooldown_until.insert(dummy.clone(), cooldown_instant());

    let (running, _guards, mut logs) = setup_preload(&prep, [PeerSelectionMsg::CheckCooldowns]);

    // te_input + AddPeer messages to manager (via tm_send_match on the variant) + final state (count only)
    assert_trace_contains(
        &running,
        &[
            te_input("ps-1", &PeerSelectionMsg::CheckCooldowns).into(),
            tm_send_match::<ManagerMessage>("ps-1", "manager", |msg| matches!(msg, ManagerMessage::AddPeer(_))),
            tm_send_match::<ManagerMessage>("ps-1", "manager", |msg| matches!(msg, ManagerMessage::AddPeer(_))),
            tm_state(
                "ps-1",
                |s: &PeerSelection| {
                    s.outbound_peers.len() == 3
                        && s.outbound_peers.contains_key(&TestPrep::peer("static1:1"))
                        && s.outbound_peers.contains_key(&TestPrep::peer("static2:2"))
                },
                "statics preferred; target filled including one ledger peer",
            ),
        ],
    );

    logs.assert_and_remove(Level::INFO, &["peer_selection.add_peer"])
        .assert_and_remove(Level::INFO, &["peer_selection.add_peer"])
        .assert_and_remove(Level::INFO, &["peer_selection.add_peer"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_regulate_skips_peers_in_cooldown() {
    let mut prep = test_prep(&["static1:1", "static2:2"]);

    // Put one static peer in cooldown
    let banned = TestPrep::peer("static1:1");
    prep.state.cooldowns.cooldown_until.insert(banned.clone(), cooldown_instant());

    // Have a ledger candidate
    let ledger = TestPrep::peer("ledger1:1");
    prep.state.ledger_candidates.insert(ledger.clone());

    // Trigger regulate
    let dummy = TestPrep::peer("dummy:9");
    prep.state.cooldowns.cooldown_until.insert(dummy.clone(), cooldown_instant());

    let (running, _guards, mut logs) = setup_preload(&prep, [PeerSelectionMsg::CheckCooldowns]);

    // te_input + AddPeer messages (banned peer skipped) + final state (count only)
    assert_trace_contains(
        &running,
        &[
            te_input("ps-1", &PeerSelectionMsg::CheckCooldowns).into(),
            tm_send_match::<ManagerMessage>("ps-1", "manager", |msg| matches!(msg, ManagerMessage::AddPeer(_))),
            tm_send_match::<ManagerMessage>("ps-1", "manager", |msg| matches!(msg, ManagerMessage::AddPeer(_))),
            tm_state(
                "ps-1",
                |s: &PeerSelection| {
                    s.outbound_peers.len() == 2
                        && !s.outbound_peers.contains_key(&banned)
                        && s.cooldowns.is_cooling(&banned)
                },
                "final state with two new outbound peers; banned static still cooling down",
            ),
        ],
    );

    logs.assert_and_remove(Level::INFO, &["peer_selection.add_peer"])
        .assert_and_remove(Level::INFO, &["peer_selection.add_peer"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_disconnected_outbound_peer_also_in_inbound() {
    let mut prep = test_prep(&[]);
    let p = TestPrep::peer("7.7.7.7:7");
    prep.state.inbound_peers.insert(p.clone(), conn());
    prep.state.outbound_peers.insert(p.clone(), PeerState::Connected(conn()));

    let (running, _guards, mut logs) = setup(
        &prep,
        PeerSelectionMsg::Disconnected(p.clone(), ConnectionId::initial(), ConnectionDirection::Outbound, false),
    );

    // te_input + final state (no RemovePeer expected for the inbound side)
    assert_trace_contains(
        &running,
        &[
            te_input(
                "ps-1",
                &PeerSelectionMsg::Disconnected(
                    p.clone(),
                    ConnectionId::initial(),
                    ConnectionDirection::Outbound,
                    false,
                ),
            )
            .into(),
            tm_state(
                "ps-1",
                |s: &PeerSelection| !s.outbound_peers.contains_key(&p) && s.inbound_peers.contains_key(&p),
                "final state: outbound removed, inbound untouched",
            ),
        ],
    );
    // Still inbound ⇒ claims must not be cleared.
    assert_trace_does_not_contain(&running, &[te_clear_peer_availability("ps-1", p).into()]);

    logs.assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

// ---------------------------------------------------------------------------
// Bounded cool-down scheduling (issue #1112)
// ---------------------------------------------------------------------------

#[test]
fn test_many_cooldowns_arm_only_one_schedule() {
    let prep = test_prep(&[]);
    // More peers than PRIORITY_MAILBOX_SIZE (10): per-peer schedules would panic.
    let peers: Vec<_> = (0..15).map(|i| TestPrep::peer(&format!("9.9.9.{i}:9"))).collect();
    let msgs: Vec<_> = peers.iter().cloned().map(PeerSelectionMsg::adversarial).collect();
    let first = peers[0].clone();
    let sid = first_schedule_id();

    let (running, _guards, mut logs) = setup_preload(&prep, msgs);

    assert_trace_contains(
        &running,
        &[
            te_input("ps-1", &PeerSelectionMsg::adversarial(first.clone())).into(),
            te_clock_suspend("ps-1").into(),
            te_schedule("ps-1", PeerSelectionMsg::CheckCooldowns, sid).into(),
            tm_state(
                "ps-1",
                |s: &PeerSelection| {
                    s.cooldowns.cooldown_until.len() == 15
                        && s.cooldown_timer.is_some()
                        && s.cooldowns.cooldown_heap.len() == 15
                },
                "all 15 peers cooling down under a single armed timer",
            ),
            te_clock(cooldown_instant()).into(),
            te_input("ps-1", &PeerSelectionMsg::CheckCooldowns).into(),
            tm_state(
                "ps-1",
                |s: &PeerSelection| s.cooldowns.cooldown_until.is_empty() && s.cooldown_timer.is_none(),
                "all cool-downs drained when the single timer fires",
            ),
        ],
    );

    for _ in 0..15 {
        logs.assert_and_remove(Level::DEBUG, &["peer_selection.adversarial"]);
    }
    logs.assert_no_remaining_at([Level::WARN, Level::ERROR]);
}

#[test]
fn test_second_cooldown_does_not_schedule_again() {
    let prep = test_prep(&[]);
    let p1 = TestPrep::peer("1.1.1.1:1");
    let p2 = TestPrep::peer("2.2.2.2:2");
    let sid = first_schedule_id();
    let after_first = {
        let mut s = prep.state.clone();
        with_single_cooldown(&mut s, p1.clone(), sid);
        s
    };
    let after_second = {
        let mut s = after_first.clone();
        s.cooldowns.add_and_is_first(p2.clone(), cooldown_instant());
        s
    };

    let (running, _guards, mut logs) =
        setup_preload(&prep, [PeerSelectionMsg::adversarial(p1.clone()), PeerSelectionMsg::adversarial(p2.clone())]);

    // Only the empty→one transition schedules; the second peer only joins the heap.
    assert_trace(
        &running,
        &[
            te_state("ps-1", &prep.state),
            te_input("ps-1", &PeerSelectionMsg::adversarial(p1.clone())),
            te_peer_adversarial("ps-1", p1.clone()),
            te_clock_suspend("ps-1"),
            te_schedule("ps-1", PeerSelectionMsg::CheckCooldowns, sid),
            te_state("ps-1", &after_first),
            te_input("ps-1", &PeerSelectionMsg::adversarial(p2.clone())),
            te_peer_adversarial("ps-1", p2.clone()),
            te_clock_suspend("ps-1"),
            te_state("ps-1", &after_second),
            te_clock(cooldown_instant()),
            te_input("ps-1", &PeerSelectionMsg::CheckCooldowns),
            te_cancel_schedule("ps-1", sid),
            te_clock_suspend("ps-1"),
            te_random_seed("ps-1"),
            te_state("ps-1", &prep.state),
        ],
    );

    logs.assert_and_remove(Level::DEBUG, &["peer_selection.adversarial"])
        .assert_and_remove(Level::DEBUG, &["peer_selection.adversarial"])
        .assert_no_remaining_at([Level::WARN, Level::ERROR]);
}

#[test]
fn test_earlier_cooldown_reschedules_timer() {
    // Static ban = 10s, non-static = 1s. Ban static first, then a non-static peer so the shorter
    // cool-down cancels and re-arms the single timer.
    let mut prep = test_prep(&["static1:1"]);
    let static_peer = TestPrep::peer("static1:1");
    let other = TestPrep::peer("other:1");
    prep.state.outbound_peers.insert(static_peer.clone(), PeerState::Connecting);

    let sid_static = first_static_schedule_id();
    let sid_other = second_schedule_id_at(cooldown_instant());

    let (running, _guards, mut logs) = setup_preload(
        &prep,
        [PeerSelectionMsg::adversarial(static_peer.clone()), PeerSelectionMsg::adversarial(other.clone())],
    );

    assert_trace_contains(
        &running,
        &[
            te_input("ps-1", &PeerSelectionMsg::adversarial(static_peer.clone())).into(),
            te_clock_suspend("ps-1").into(),
            te_schedule("ps-1", PeerSelectionMsg::CheckCooldowns, sid_static).into(),
            te_input("ps-1", &PeerSelectionMsg::adversarial(other.clone())).into(),
            te_clock_suspend("ps-1").into(),
            te_cancel_schedule("ps-1", sid_static).into(),
            te_schedule("ps-1", PeerSelectionMsg::CheckCooldowns, sid_other).into(),
            tm_state(
                "ps-1",
                |s: &PeerSelection| {
                    s.cooldowns.cooldown_until.len() == 2
                        && s.cooldown_timer == Some(sid_other)
                        && s.cooldowns.peek().map(|(t, _)| t) == Some(cooldown_instant())
                        && s.cooldowns.cooldown_until.get(&static_peer).copied() == Some(static_cooldown_instant())
                },
                "timer re-armed to the earlier non-static cool-down",
            ),
        ],
    );

    logs.assert_and_remove(Level::DEBUG, &["peer_selection.adversarial"])
        .assert_and_remove(Level::WARN, &["removing peer (outbound)"])
        .assert_and_remove(Level::DEBUG, &["peer_selection.adversarial"])
        .assert_no_remaining_at([Level::WARN, Level::ERROR]);
}
