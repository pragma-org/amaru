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

use amaru_ouroboros::{ConnectionDirection, ConnectionId};
use amaru_protocols::manager::ManagerMessage;
use tracing::Level;

use super::*;
use crate::stages::{
    peer_selection::test_setup::{
        TestPrep, assert_trace, cooldown_instant, first_schedule_id, second_schedule_id, setup, setup_preload,
        te_cancel_schedule, te_clock, te_schedule, te_send, test_prep_no_static, test_prep_static,
    },
    test_utils::{te_input, te_state},
};

fn conn() -> Connection {
    Connection::new(ConnectionId::initial(), true, false)
}

// ---------------------------------------------------------------------------
// Initialize
// ---------------------------------------------------------------------------

#[test]
fn test_initialize_empty_static() {
    let prep = test_prep_no_static();
    let state = prep.state.clone();
    let msg = PeerSelectionMsg::Initialize;
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(&running, &[te_state("ps-1", &state), te_input("ps-1", &msg), te_state("ps-1", &state)]);
    logs.assert_and_remove(Level::INFO, &["peer_selection.connect_initial"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

#[test]
fn test_initialize_adds_static_peers() {
    let prep = test_prep_static(&["10.0.0.1:1", "10.0.0.2:2"]);
    let p1 = TestPrep::peer("10.0.0.1:1");
    let p2 = TestPrep::peer("10.0.0.2:2");
    let state = prep.state.clone();
    let msg = PeerSelectionMsg::Initialize;
    let after = {
        let mut s = state.clone();
        s.outbound_peers.insert(p1.clone(), PeerState::Connecting);
        s.outbound_peers.insert(p2.clone(), PeerState::Connecting);
        s
    };
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("ps-1", &state),
            te_input("ps-1", &msg),
            te_send("ps-1", "manager", ManagerMessage::AddPeer(p1)),
            te_send("ps-1", "manager", ManagerMessage::AddPeer(p2)),
            te_state("ps-1", &after),
        ],
    );
    logs.assert_and_remove(Level::INFO, &["peer_selection.connect_initial"])
        .assert_no_remaining_at([Level::WARN, Level::ERROR]);
}

// ---------------------------------------------------------------------------
// AddPeer
// ---------------------------------------------------------------------------

#[test]
fn test_add_peer_not_in_cooldown() {
    let prep = test_prep_no_static();
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
    let prep = test_prep_no_static();
    let p = TestPrep::peer("8.8.8.8:8");
    let state = prep.state.clone();
    let sid = first_schedule_id();
    let after_remove = {
        let mut s = state.clone();
        s.cooldown_timers.insert(p.clone(), sid);
        s
    };
    let after_add = {
        let mut s = after_remove.clone();
        s.cooldown_timers.remove(&p);
        s.outbound_peers.insert(p.clone(), PeerState::Connecting);
        s
    };
    let (running, _guards, mut logs) =
        setup_preload(&prep, [PeerSelectionMsg::Adversarial(p.clone()), PeerSelectionMsg::AddPeer(p.clone())]);
    assert_trace(
        &running,
        &[
            te_state("ps-1", &state),
            te_input("ps-1", &PeerSelectionMsg::Adversarial(p.clone())),
            te_schedule("ps-1", PeerSelectionMsg::CooldownEnded(p.clone()), sid),
            te_state("ps-1", &after_remove),
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
    let prep = test_prep_no_static();
    let p = TestPrep::peer("7.7.7.7:7");
    let state = prep.state.clone();
    let sid = first_schedule_id();
    let after = {
        let mut s = state.clone();
        s.cooldown_timers.insert(p.clone(), sid);
        s
    };
    let (running, _guards, mut logs) = setup(&prep, PeerSelectionMsg::Adversarial(p.clone()));
    assert_trace(
        &running,
        &[
            te_state("ps-1", &state),
            te_input("ps-1", &PeerSelectionMsg::Adversarial(p.clone())),
            te_schedule("ps-1", PeerSelectionMsg::CooldownEnded(p.clone()), sid),
            te_state("ps-1", &after),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["peer_selection.adversarial"])
        .assert_no_remaining_at([Level::WARN, Level::ERROR]);
}

// ---------------------------------------------------------------------------
// CooldownEnded
// ---------------------------------------------------------------------------

#[test]
fn test_cooldown_ended_stale() {
    let prep = test_prep_no_static();
    let p = TestPrep::peer("6.6.6.6:6");
    let state = prep.state.clone();
    let msg = PeerSelectionMsg::CooldownEnded(p);
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(&running, &[te_state("ps-1", &state), te_input("ps-1", &msg), te_state("ps-1", &state)]);
    logs.assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_cooldown_ended_triggers_regulate() {
    let prep = test_prep_no_static();
    let p = TestPrep::peer("5.5.5.5:5");
    let state = prep.state.clone();
    let sid = first_schedule_id();
    let after_remove = {
        let mut s = state.clone();
        s.cooldown_timers.insert(p.clone(), sid);
        s
    };
    let (running, _guards, mut logs) =
        setup_preload(&prep, [PeerSelectionMsg::Adversarial(p.clone()), PeerSelectionMsg::CooldownEnded(p.clone())]);
    assert_trace(
        &running,
        &[
            te_state("ps-1", &state),
            te_input("ps-1", &PeerSelectionMsg::Adversarial(p.clone())),
            te_schedule("ps-1", PeerSelectionMsg::CooldownEnded(p.clone()), sid),
            te_state("ps-1", &after_remove),
            te_clock(cooldown_instant()),
            te_input("ps-1", &PeerSelectionMsg::CooldownEnded(p.clone())),
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
    let prep = test_prep_no_static();
    let p = TestPrep::peer("4.4.4.4:4");
    let state = prep.state.clone();
    let msg = PeerSelectionMsg::Connected(p.clone(), conn(), ConnectionDirection::Inbound);
    let after = {
        let mut s = state.clone();
        s.inbound_peers.insert(p.clone(), PeerState::Connected(conn()));
        s
    };
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(&running, &[te_state("ps-1", &state), te_input("ps-1", &msg), te_state("ps-1", &after)]);
    logs.assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_connected_inbound_too_many() {
    let prep = test_prep_no_static();
    let p = TestPrep::peer("3.3.3.3:3");
    let state = prep.state.clone();
    let mut state_full = state.clone();
    for i in 0..10u8 {
        state_full.inbound_peers.insert(TestPrep::peer(&format!("1.1.1.{i}:1")), PeerState::Connected(conn()));
    }
    let msg = PeerSelectionMsg::Connected(p.clone(), conn(), ConnectionDirection::Inbound);
    let (running, _guards, mut logs) = setup_preload(&prep, [msg.clone()]);
    assert_trace(&running, &[te_state("ps-1", &state_full), te_input("ps-1", &msg), te_state("ps-1", &state_full)]);
    logs.assert_and_remove(Level::INFO, &["rejecting inbound connection"])
        .assert_no_remaining_at([Level::WARN, Level::ERROR]);
}

#[test]
fn test_connected_outbound() {
    let prep = test_prep_no_static();
    let p = TestPrep::peer("2.2.2.2:2");
    let state = prep.state.clone();
    let msg = PeerSelectionMsg::Connected(p.clone(), conn(), ConnectionDirection::Outbound);
    let after = {
        let mut s = state.clone();
        s.outbound_peers.insert(p.clone(), PeerState::Connected(conn()));
        s
    };
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(&running, &[te_state("ps-1", &state), te_input("ps-1", &msg), te_state("ps-1", &after)]);
    logs.assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_disconnected_inbound() {
    let prep = test_prep_no_static();
    let p = TestPrep::peer("1.1.1.1:1");
    let state = prep.state.clone();
    let mut state_with_peer = state.clone();
    state_with_peer.inbound_peers.insert(p.clone(), PeerState::Connected(conn()));
    let msg = PeerSelectionMsg::Disconnected(p.clone(), ConnectionId::initial(), ConnectionDirection::Inbound);
    let after = {
        let mut s = state_with_peer.clone();
        s.inbound_peers.remove(&p);
        s
    };
    let (running, _guards, mut logs) = setup_preload(&prep, [msg.clone()]);
    assert_trace(&running, &[te_state("ps-1", &state_with_peer), te_input("ps-1", &msg), te_state("ps-1", &after)]);
    logs.assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_disconnected_outbound_connecting_schedules_cooldown() {
    let prep = test_prep_no_static();
    let p = TestPrep::peer("0.0.0.0:0");
    let state = prep.state.clone();
    let mut state_conn = state.clone();
    state_conn.outbound_peers.insert(p.clone(), PeerState::Connecting);
    let msg = PeerSelectionMsg::Disconnected(p.clone(), ConnectionId::initial(), ConnectionDirection::Outbound);
    let sid = first_schedule_id();
    let after = {
        let mut s = state_conn.clone();
        s.outbound_peers.remove(&p);
        s.cooldown_timers.insert(p.clone(), sid);
        s
    };
    let (running, _guards, mut logs) = setup_preload(&prep, [msg.clone()]);
    assert_trace(
        &running,
        &[
            te_state("ps-1", &state_conn),
            te_input("ps-1", &msg),
            te_schedule("ps-1", PeerSelectionMsg::CooldownEnded(p.clone()), sid),
            te_state("ps-1", &after),
        ],
    );
    logs.assert_no_remaining_at([Level::WARN, Level::ERROR]);
}

// ---------------------------------------------------------------------------
// Double-remove (Adversarial twice)
// ---------------------------------------------------------------------------

#[test]
fn test_adversarial_twice_cancels_and_reschedules() {
    let prep = test_prep_no_static();
    let p = TestPrep::peer("11.11.11.11:11");
    let state = prep.state.clone();
    let sid0 = first_schedule_id();
    let sid1 = second_schedule_id();
    let after_first = {
        let mut s = state.clone();
        s.cooldown_timers.insert(p.clone(), sid0);
        s
    };
    let after_second = {
        let mut s = after_first.clone();
        s.cooldown_timers.insert(p.clone(), sid1);
        s
    };
    let (running, _guards, mut logs) =
        setup_preload(&prep, [PeerSelectionMsg::Adversarial(p.clone()), PeerSelectionMsg::Adversarial(p.clone())]);
    assert_trace(
        &running,
        &[
            te_state("ps-1", &state),
            te_input("ps-1", &PeerSelectionMsg::Adversarial(p.clone())),
            te_schedule("ps-1", PeerSelectionMsg::CooldownEnded(p.clone()), sid0),
            te_state("ps-1", &after_first),
            te_input("ps-1", &PeerSelectionMsg::Adversarial(p.clone())),
            te_cancel_schedule("ps-1", sid0),
            te_schedule("ps-1", PeerSelectionMsg::CooldownEnded(p.clone()), sid1),
            te_state("ps-1", &after_second),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["peer_selection.adversarial"])
        .assert_and_remove(Level::DEBUG, &["peer_selection.adversarial"])
        .assert_no_remaining_at([Level::WARN, Level::ERROR]);
}
