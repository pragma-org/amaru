// Copyright 2026 PRAGMA
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::collections::{BTreeMap, BTreeSet};

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

#[test]
fn test_connect_initial_empty_static_peers() {
    let prep = test_prep_no_static();
    let state = prep.state.clone();
    let msg = PeerSelectionMsg::ConnectInitial;
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(&running, &[te_state("ps-1", &state), te_input("ps-1", &msg), te_state("ps-1", &state)]);
    logs.assert_and_remove(Level::INFO, &["peer_selection.connect_initial"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

#[test]
fn test_connect_initial_adds_each_static_peer_in_order() {
    let prep = test_prep_static(&["10.0.0.1:1", "10.0.0.2:2"]);
    let p1 = TestPrep::peer("10.0.0.1:1");
    let p2 = TestPrep::peer("10.0.0.2:2");
    let state = prep.state.clone();
    let msg = PeerSelectionMsg::ConnectInitial;
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("ps-1", &state),
            te_input("ps-1", &msg),
            te_send("ps-1", "manager", ManagerMessage::AddPeer(p1.clone())),
            te_send("ps-1", "manager", ManagerMessage::AddPeer(p2.clone())),
            te_state("ps-1", &state),
        ],
    );
    logs.assert_and_remove(Level::INFO, &["peer_selection.connect_initial"])
        .assert_no_remaining_at([Level::WARN, Level::ERROR]);
}

#[test]
fn test_add_peer_forwards_when_not_in_cooldown() {
    let prep = test_prep_no_static();
    let p = TestPrep::peer("9.9.9.9:9");
    let state = prep.state.clone();
    let msg = PeerSelectionMsg::AddPeer(p.clone());
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("ps-1", &state),
            te_input("ps-1", &msg),
            te_send("ps-1", "manager", ManagerMessage::AddPeer(p)),
            te_state("ps-1", &state),
        ],
    );
    logs.assert_and_remove(Level::INFO, &["peer_selection.add_peer"])
        .assert_no_remaining_at([Level::WARN, Level::ERROR]);
}

#[test]
fn test_remove_peer_forwards_and_schedules_cooldown() {
    let prep = test_prep_no_static();
    let p = TestPrep::peer("8.8.8.8:8");
    let state = prep.state.clone();
    let msg = PeerSelectionMsg::RemovePeer(p.clone());
    let sid = first_schedule_id();
    let in_cooldown = prep.state_with_timers(BTreeMap::from([(p.clone(), sid)]));
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("ps-1", &state),
            te_input("ps-1", &msg),
            te_send("ps-1", "manager", ManagerMessage::RemovePeer(p.clone())),
            te_schedule("ps-1", PeerSelectionMsg::CooldownEnded(p.clone()), sid),
            te_state("ps-1", &in_cooldown),
            te_clock(cooldown_instant()),
            te_input("ps-1", &PeerSelectionMsg::CooldownEnded(p.clone())),
            te_state("ps-1", &state),
        ],
    );
    logs.assert_and_remove(Level::INFO, &["peer_selection.remove_peer"])
        .assert_no_remaining_at([Level::WARN, Level::ERROR]);
}

#[test]
fn test_remove_peer_static_warns_and_cooldown_ended_readds() {
    let prep = test_prep_static(&["7.7.7.7:7"]);
    let p = TestPrep::peer("7.7.7.7:7");
    let state = prep.state.clone();
    let msg = PeerSelectionMsg::RemovePeer(p.clone());
    let sid = first_schedule_id();
    let in_cooldown = prep.state_with_timers(BTreeMap::from([(p.clone(), sid)]));
    let after_readd = prep.state.clone();
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("ps-1", &state),
            te_input("ps-1", &msg),
            te_send("ps-1", "manager", ManagerMessage::RemovePeer(p.clone())),
            te_schedule("ps-1", PeerSelectionMsg::CooldownEnded(p.clone()), sid),
            te_state("ps-1", &in_cooldown),
            te_clock(cooldown_instant()),
            te_input("ps-1", &PeerSelectionMsg::CooldownEnded(p.clone())),
            te_send("ps-1", "manager", ManagerMessage::AddPeer(p.clone())),
            te_state("ps-1", &after_readd),
        ],
    );
    logs.assert_and_remove(Level::INFO, &["peer_selection.remove_peer"])
        .assert_and_remove(Level::WARN, &["removing static peer"])
        .assert_and_remove(Level::INFO, &["peer_selection.reconnect_static_peer"])
        .assert_no_remaining_at([Level::WARN, Level::ERROR]);
}

#[test]
fn test_cooldown_ended_non_static_does_not_readd() {
    let prep = test_prep_no_static();
    let p = TestPrep::peer("6.6.6.6:6");
    let state = prep.state.clone();
    let msg = PeerSelectionMsg::RemovePeer(p.clone());
    let sid = first_schedule_id();
    let in_cooldown = prep.state_with_timers(BTreeMap::from([(p.clone(), sid)]));
    let after_clear = prep.state.clone();
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("ps-1", &state),
            te_input("ps-1", &msg),
            te_send("ps-1", "manager", ManagerMessage::RemovePeer(p.clone())),
            te_schedule("ps-1", PeerSelectionMsg::CooldownEnded(p.clone()), sid),
            te_state("ps-1", &in_cooldown),
            te_clock(cooldown_instant()),
            te_input("ps-1", &PeerSelectionMsg::CooldownEnded(p.clone())),
            te_state("ps-1", &after_clear),
        ],
    );
    logs.assert_and_remove(Level::INFO, &["peer_selection.remove_peer"])
        .assert_no_remaining_at([Level::WARN, Level::ERROR]);
}

#[test]
fn test_cooldown_ended_stale_ignored() {
    let prep = test_prep_no_static();
    let p = TestPrep::peer("5.5.5.5:5");
    let state = prep.state.clone();
    let msg = PeerSelectionMsg::CooldownEnded(p);
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(&running, &[te_state("ps-1", &state), te_input("ps-1", &msg), te_state("ps-1", &state)]);
    logs.assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_add_peer_during_cooldown_skipped() {
    let prep = test_prep_no_static();
    let p = TestPrep::peer("4.4.4.4:4");
    let state = prep.state.clone();
    let sid = first_schedule_id();
    let after_remove = prep.state_with_timers(BTreeMap::from([(p.clone(), sid)]));
    let (running, _guards, mut logs) =
        setup_preload(&prep, [PeerSelectionMsg::RemovePeer(p.clone()), PeerSelectionMsg::AddPeer(p.clone())]);
    assert_trace(
        &running,
        &[
            te_state("ps-1", &state),
            te_input("ps-1", &PeerSelectionMsg::RemovePeer(p.clone())),
            te_send("ps-1", "manager", ManagerMessage::RemovePeer(p.clone())),
            te_schedule("ps-1", PeerSelectionMsg::CooldownEnded(p.clone()), sid),
            te_state("ps-1", &after_remove),
            te_input("ps-1", &PeerSelectionMsg::AddPeer(p.clone())),
            te_state("ps-1", &after_remove),
            te_clock(cooldown_instant()),
            te_input("ps-1", &PeerSelectionMsg::CooldownEnded(p.clone())),
            te_state("ps-1", &state),
        ],
    );
    logs.assert_and_remove(Level::INFO, &["peer_selection.remove_peer"])
        .assert_and_remove(Level::INFO, &["peer_selection.add_peer"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_remove_peer_twice_cancels_and_reschedules() {
    let prep = test_prep_no_static();
    let p = TestPrep::peer("3.3.3.3:3");
    let state = prep.state.clone();
    let sid0 = first_schedule_id();
    let sid1 = second_schedule_id();
    let after_second = prep.state_with_timers(BTreeMap::from([(p.clone(), sid1)]));
    let (running, _guards, mut logs) =
        setup_preload(&prep, [PeerSelectionMsg::RemovePeer(p.clone()), PeerSelectionMsg::RemovePeer(p.clone())]);
    assert_trace(
        &running,
        &[
            te_state("ps-1", &state),
            te_input("ps-1", &PeerSelectionMsg::RemovePeer(p.clone())),
            te_send("ps-1", "manager", ManagerMessage::RemovePeer(p.clone())),
            te_schedule("ps-1", PeerSelectionMsg::CooldownEnded(p.clone()), sid0),
            te_state("ps-1", &prep.state_with_timers(BTreeMap::from([(p.clone(), sid0)]))),
            te_input("ps-1", &PeerSelectionMsg::RemovePeer(p.clone())),
            te_cancel_schedule("ps-1", sid0),
            te_send("ps-1", "manager", ManagerMessage::RemovePeer(p.clone())),
            te_schedule("ps-1", PeerSelectionMsg::CooldownEnded(p.clone()), sid1),
            te_state("ps-1", &after_second),
            te_clock(cooldown_instant()),
            te_input("ps-1", &PeerSelectionMsg::CooldownEnded(p.clone())),
            te_state("ps-1", &state),
        ],
    );
    logs.assert_and_remove(Level::INFO, &["peer_selection.remove_peer"])
        .assert_and_remove(Level::INFO, &["peer_selection.remove_peer"])
        .assert_no_remaining_at([Level::WARN, Level::ERROR]);
}

#[test]
fn test_downstream_connected_first_time() {
    let prep = test_prep_no_static();
    let p = TestPrep::peer("2.2.2.2:2");
    let state = prep.state.clone();
    let msg = PeerSelectionMsg::DownstreamConnected(p.clone());
    let after: PeerSelection = prep.state_with_downstream([p].into_iter().collect());
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(&running, &[te_state("ps-1", &state), te_input("ps-1", &msg), te_state("ps-1", &after)]);
    logs.assert_and_remove(Level::INFO, &["peer_selection.downstream_connected"])
        .assert_no_remaining_at([Level::WARN, Level::ERROR]);
}

#[test]
fn test_downstream_connected_second_is_silent() {
    let prep = test_prep_no_static();
    let p = TestPrep::peer("1.1.1.1:1");
    let initial = prep.state.clone();
    let state = prep.state_with_downstream(BTreeSet::from([p.clone()]));
    let msg = PeerSelectionMsg::DownstreamConnected(p.clone());
    let (running, _guards, mut logs) = setup_preload(
        &prep,
        [PeerSelectionMsg::DownstreamConnected(p.clone()), PeerSelectionMsg::DownstreamConnected(p)],
    );
    assert_trace(
        &running,
        &[
            te_state("ps-1", &initial),
            te_input("ps-1", &PeerSelectionMsg::DownstreamConnected(TestPrep::peer("1.1.1.1:1"))),
            te_state("ps-1", &state),
            te_input("ps-1", &msg),
            te_state("ps-1", &state),
        ],
    );
    logs.assert_and_remove(Level::INFO, &["peer_selection.downstream_connected"]).assert_no_remaining_at([
        Level::INFO,
        Level::WARN,
        Level::ERROR,
    ]);
}

#[test]
fn test_downstream_connected_then_remove_peer_preserves_downstream_after_cooldown() {
    let prep = test_prep_no_static();
    let p = TestPrep::peer("192.168.0.1:7777");
    let initial = prep.state.clone();
    let sid = first_schedule_id();
    let after_downstream = prep.state_with_downstream(BTreeSet::from([p.clone()]));
    let in_cooldown = PeerSelection {
        manager: prep.manager.clone(),
        static_peers: prep.state.static_peers.clone(),
        peer_removal_cooldown: prep.state.peer_removal_cooldown,
        cooldown_timers: BTreeMap::from([(p.clone(), sid)]),
        downstream_connected: BTreeSet::from([p.clone()]),
    };
    let (running, _guards, mut logs) = setup_preload(
        &prep,
        [PeerSelectionMsg::DownstreamConnected(p.clone()), PeerSelectionMsg::RemovePeer(p.clone())],
    );
    assert_trace(
        &running,
        &[
            te_state("ps-1", &initial),
            te_input("ps-1", &PeerSelectionMsg::DownstreamConnected(p.clone())),
            te_state("ps-1", &after_downstream),
            te_input("ps-1", &PeerSelectionMsg::RemovePeer(p.clone())),
            te_send("ps-1", "manager", ManagerMessage::RemovePeer(p.clone())),
            te_schedule("ps-1", PeerSelectionMsg::CooldownEnded(p.clone()), sid),
            te_state("ps-1", &in_cooldown),
            te_clock(cooldown_instant()),
            te_input("ps-1", &PeerSelectionMsg::CooldownEnded(p.clone())),
            te_state("ps-1", &after_downstream),
        ],
    );
    logs.assert_and_remove(Level::INFO, &["peer_selection.downstream_connected"])
        .assert_and_remove(Level::INFO, &["peer_selection.remove_peer"])
        .assert_no_remaining_at([Level::WARN, Level::ERROR]);
}
