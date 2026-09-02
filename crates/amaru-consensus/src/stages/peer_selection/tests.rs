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

use amaru_observability::tracing::Level;
use amaru_ouroboros::{ConnectionDirection, ConnectionId};
use amaru_protocols::{connection::LocalUse, manager::ManagerMessage};
use amaru_pure_stage::{
    simulation::Run,
    trace_match::{assert_trace_contains, assert_trace_does_not_contain, tm_send_match},
};

use super::*;
use crate::stages::{
    peer_selection::test_setup::{
        TestPrep, cooldown_duration, cooldown_instant, first_schedule_id, first_static_schedule_id,
        peer_selection_stage, second_schedule_id_at, setup, setup_preload, setup_preload_until_sleeping, sim_at,
        sim_t0, static_cooldown_instant, te_cancel_schedule, te_clear_peer_availability, te_clock, te_clock_suspend,
        te_is_static_peer, te_peer_adversarial, te_random_seed, te_rank_peers_for_churn, te_record_advertisability,
        te_record_connection_failure, te_schedule, te_send, test_prep, test_prep_with_snapshot,
        tm_add_stage_starts_with, with_single_cooldown,
    },
    test_utils::{assert_trace, te_input, te_state, tm_state},
};

fn conn() -> Connection {
    Connection::new(ConnectionId::initial(), true, false)
}

fn using_conn() -> Connection {
    conn().with_local_use(LocalUse::Diffusion)
}

// ---------------------------------------------------------------------------
// Initialize
// ---------------------------------------------------------------------------

#[test]
fn test_initialize_empty_static() {
    let prep = test_prep(&[]);
    let state = prep.state.clone();
    let msg = PeerSelectionMsg::Initialize;
    let (running, _guards, mut logs) = setup_preload_until_sleeping(&prep, [msg.clone()]);

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

    logs.assert_and_remove(Level::INFO, &["peer_selection.connect_initial", "static_peers=0", "snapshot_peers=0"])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_initialize_adds_static_peers() {
    let prep = test_prep(&["10.0.0.1:1", "10.0.0.2:2"]);
    let mut state = prep.state.clone();
    let msg = PeerSelectionMsg::Initialize;

    let p1 = TestPrep::peer("10.0.0.1:1");
    let p2 = TestPrep::peer("10.0.0.2:2");

    state.outbound_peers.insert(p1, PeerState::Connecting);
    state.outbound_peers.insert(p2, PeerState::Connecting);

    let (running, _guards, mut logs) = setup_preload_until_sleeping(&prep, [msg.clone()]);

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

    logs.assert_and_remove(Level::INFO, &["peer_selection.connect_initial", "static_peers=2", "snapshot_peers=0"])
        .assert_and_remove(Level::INFO, &["peer_selection.peer.added", r#"peer="10.0.0.1:1""#])
        .assert_and_remove(Level::INFO, &["peer_selection.peer.added", r#"peer="10.0.0.2:2""#])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_initialize_resolves_static_hostname() {
    use std::collections::{BTreeMap, BTreeSet};

    use amaru_kernel::PeerCandidate;
    use amaru_pure_stage::trace_match::tm_external_effect;

    use crate::effects::ResolvePeerCandidate;

    let resolved = TestPrep::peer("10.9.9.9:3001");
    let candidate = PeerCandidate::host("relay.example".parse().unwrap(), 3001);
    let mut prep = test_prep(&[]);
    prep.extra_static.insert(candidate.clone());
    prep.resolve = BTreeMap::from([(candidate.clone(), BTreeSet::from([resolved]))]);

    let mut state = prep.state.clone();
    state.outbound_peers.insert(resolved, PeerState::Connecting);
    state.bound.insert(candidate.clone(), resolved);
    let msg = PeerSelectionMsg::Initialize;
    let (running, _guards, mut logs) = setup_preload_until_sleeping(&prep, [msg.clone()]);

    assert_trace_contains(
        &running,
        &[
            te_input("ps-1", &msg).into(),
            tm_external_effect::<ResolvePeerCandidate>("ps-1"),
            tm_send_match::<ManagerMessage>(
                "ps-1",
                "manager",
                |m| matches!(m, ManagerMessage::AddPeer(p) if *p == resolved),
            ),
            te_state("ps-1", &state).into(),
        ],
    );

    logs.assert_and_remove(Level::INFO, &["peer_selection.connect_initial", "static_peers=1", "snapshot_peers=0"])
        .assert_and_remove(
            Level::INFO,
            &["peer_selection.peer.resolved", r#"candidate="relay.example:3001""#, r#"peer="10.9.9.9:3001""#],
        )
        .assert_and_remove(Level::INFO, &["peer_selection.peer.added", r#"peer="10.9.9.9:3001""#])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_initialize_resolves_static_srv() {
    use std::collections::{BTreeMap, BTreeSet};

    use amaru_kernel::PeerCandidate;
    use amaru_pure_stage::trace_match::tm_external_effect;

    use crate::effects::ResolvePeerCandidate;

    let resolved = TestPrep::peer("10.8.8.8:6000");
    let candidate = PeerCandidate::srv("pool.example".parse().unwrap());
    let mut prep = test_prep(&[]);
    prep.extra_static.insert(candidate.clone());
    prep.resolve = BTreeMap::from([(candidate.clone(), BTreeSet::from([resolved]))]);

    let mut state = prep.state.clone();
    state.outbound_peers.insert(resolved, PeerState::Connecting);
    state.bound.insert(candidate.clone(), resolved);
    let msg = PeerSelectionMsg::Initialize;
    let (running, _guards, mut logs) = setup_preload_until_sleeping(&prep, [msg.clone()]);

    assert_trace_contains(
        &running,
        &[
            te_input("ps-1", &msg).into(),
            tm_external_effect::<ResolvePeerCandidate>("ps-1"),
            tm_send_match::<ManagerMessage>(
                "ps-1",
                "manager",
                |m| matches!(m, ManagerMessage::AddPeer(p) if *p == resolved),
            ),
            te_state("ps-1", &state).into(),
        ],
    );

    logs.assert_and_remove(Level::INFO, &["peer_selection.connect_initial", "static_peers=1", "snapshot_peers=0"])
        .assert_and_remove(
            Level::INFO,
            &["peer_selection.peer.resolved", r#"candidate="pool.example""#, r#"peer="10.8.8.8:6000""#],
        )
        .assert_and_remove(Level::INFO, &["peer_selection.peer.added", r#"peer="10.8.8.8:6000""#])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_initialize_resolve_failure_does_not_dial() {
    use amaru_kernel::PeerCandidate;
    use amaru_pure_stage::trace_match::tm_external_effect;

    use crate::effects::ResolvePeerCandidate;

    let candidate = PeerCandidate::host("missing.example".parse().unwrap(), 3001);
    let mut prep = test_prep(&[]);
    prep.extra_static.insert(candidate.clone());
    let msg = PeerSelectionMsg::Initialize;
    // UntilSleeping: a failed lookup arms a delayed Regulate; UntilBlocked would
    // keep advancing that retry timer forever under a frozen DNS override.
    let (running, _guards, mut logs) = setup_preload_until_sleeping(&prep, [msg.clone()]);

    let mut state = prep.state.clone();
    state.resolve_backoff.insert(candidate, sim_at(RESOLUTION_RETRY_DELAY));

    assert_trace_contains(
        &running,
        &[
            te_input("ps-1", &msg).into(),
            tm_external_effect::<ResolvePeerCandidate>("ps-1"),
            te_state("ps-1", &state).into(),
        ],
    );
    assert_trace_does_not_contain(
        &running,
        &[tm_send_match::<ManagerMessage>("ps-1", "manager", |m| matches!(m, ManagerMessage::AddPeer(_)))],
    );

    logs.assert_and_remove(Level::INFO, &["peer_selection.connect_initial", "static_peers=1", "snapshot_peers=0"])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_initialize_fills_from_snapshot() {
    let prep = test_prep_with_snapshot(&[], &["10.0.2.1:1", "10.0.2.2:2", "10.0.2.3:3"]);
    let msg = PeerSelectionMsg::Initialize;
    let (running, _guards, mut logs) = setup_preload_until_sleeping(&prep, [msg.clone()]);

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

    logs.assert_and_remove(Level::INFO, &["peer_selection.connect_initial", "static_peers=0", "snapshot_peers=3"])
        .assert_and_remove(Level::INFO, &["peer_selection.peer.added", r#"peer="10.0.2.3:3""#])
        .assert_and_remove(Level::INFO, &["peer_selection.peer.added", r#"peer="10.0.2.2:2""#])
        .assert_and_remove(Level::INFO, &["peer_selection.peer.added", r#"peer="10.0.2.1:1""#])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_regulate_prefers_static_before_snapshot_before_ledger() {
    let mut prep = test_prep_with_snapshot(&["10.0.1.1:1"], &["10.0.2.1:1", "10.0.2.2:2"]).with_ledger(&["10.0.3.1:1"]);
    // Start with empty outbound and trigger regulate via CheckCooldowns.
    let dummy = TestPrep::peer("10.0.9.9:9");
    prep.state.cooldowns.cooldown_until.insert(dummy, cooldown_instant());

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
                    s.outbound_peers.contains_key(&TestPrep::peer("10.0.1.1:1")) && s.outbound_peers.len() == 3
                },
                "static preferred and target filled",
            ),
        ],
    );

    logs.assert_and_remove(Level::INFO, &["peer_selection.peer.added", r#"peer="10.0.1.1:1""#])
        .assert_and_remove(Level::INFO, &["peer_selection.peer.added", r#"peer="10.0.2.2:2""#])
        .assert_and_remove(Level::INFO, &["peer_selection.peer.added", r#"peer="10.0.2.1:1""#])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_ledger_candidates_replace_does_not_clear_snapshot() {
    // Ledger candidates are written to Performance by the helper stage (or test prep);
    // Regulate only refills. Snapshot pool is independent of that update.
    let prep = test_prep_with_snapshot(&[], &["10.0.2.1:1"]).with_ledger(&["10.0.3.1:1"]);
    let msg = PeerSelectionMsg::Regulate;

    let (running, _guards, mut logs) = setup(&prep, msg.clone());

    assert_trace_contains(
        &running,
        &[
            te_input("ps-1", &msg).into(),
            te_random_seed("ps-1").into(),
            tm_state(
                "ps-1",
                |s: &PeerSelection| {
                    s.outbound_peers.len() == 2
                        && s.outbound_peers.contains_key(&TestPrep::peer("10.0.2.1:1"))
                        && s.outbound_peers.contains_key(&TestPrep::peer("10.0.3.1:1"))
                },
                "snapshot retained alongside ledger after regulate",
            ),
        ],
    );

    logs.assert_and_remove(Level::INFO, &["peer_selection.peer.added", r#"peer="10.0.2.1:1""#])
        .assert_and_remove(Level::INFO, &["peer_selection.peer.added", r#"peer="10.0.3.1:1""#])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

// ---------------------------------------------------------------------------
// AddPeer
// ---------------------------------------------------------------------------

#[test]
fn test_add_peer_not_in_cooldown() {
    let prep = test_prep(&[]);
    let p = TestPrep::peer("9.9.9.9:9");
    let state = prep.state.clone();
    let msg = PeerSelectionMsg::AddPeer(p);
    let after = {
        let mut s = state.clone();
        s.outbound_peers.insert(p, PeerState::Connecting);
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
    logs.assert_and_remove(Level::INFO, &["peer_selection.peer.added", r#"peer="9.9.9.9:9""#, "was_banned=false"])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_add_peer_during_cooldown_cancels_timer() {
    let prep = test_prep(&[]);
    let p = TestPrep::peer("8.8.8.8:8");
    let state = prep.state.clone();
    let sid = first_schedule_id();
    let after_ban = {
        let mut s = state.clone();
        with_single_cooldown(&mut s, p, sid);
        s
    };
    let after_add = {
        let mut s = state.clone();
        s.outbound_peers.insert(p, PeerState::Connecting);
        s
    };
    let (running, _guards, mut logs) =
        setup_preload(&prep, [PeerSelectionMsg::adversarial(p), PeerSelectionMsg::AddPeer(p)]);
    assert_trace(
        &running,
        &[
            te_state("ps-1", &state),
            te_input("ps-1", &PeerSelectionMsg::adversarial(p)),
            te_is_static_peer("ps-1", p),
            te_clock_suspend("ps-1"),
            te_peer_adversarial("ps-1", p),
            te_schedule("ps-1", PeerSelectionMsg::CheckCooldowns, sid),
            te_state("ps-1", &after_ban),
            te_input("ps-1", &PeerSelectionMsg::AddPeer(p)),
            te_cancel_schedule("ps-1", sid),
            te_send("ps-1", "manager", ManagerMessage::AddPeer(p)),
            te_state("ps-1", &after_add),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["peer_selection.peer.adversarial", r#"peer="8.8.8.8:8""#])
        .assert_and_remove(Level::INFO, &["peer_selection.peer.added", r#"peer="8.8.8.8:8""#, "was_banned=true"])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
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
        with_single_cooldown(&mut s, p, sid);
        s
    };
    let (running, _guards, mut logs) = setup(&prep, PeerSelectionMsg::adversarial(p));
    assert_trace_contains(
        &running,
        &[
            te_state("ps-1", &state).into(),
            te_input("ps-1", &PeerSelectionMsg::adversarial(p)).into(),
            te_is_static_peer("ps-1", p).into(),
            te_clock_suspend("ps-1").into(),
            te_peer_adversarial("ps-1", p).into(),
            te_schedule("ps-1", PeerSelectionMsg::CheckCooldowns, sid).into(),
            te_state("ps-1", &after).into(),
            te_clock(cooldown_instant()).into(),
            te_input("ps-1", &PeerSelectionMsg::CheckCooldowns).into(),
            te_cancel_schedule("ps-1", sid).into(),
            te_clock_suspend("ps-1").into(),
            te_random_seed("ps-1").into(),
            te_state("ps-1", &state).into(),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["peer_selection.peer.adversarial", r#"peer="7.7.7.7:7""#])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
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
    assert_trace_contains(
        &running,
        &[te_input("ps-1", &msg).into(), te_random_seed("ps-1").into(), te_state("ps-1", &state).into()],
    );
    logs.assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
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
        with_single_cooldown(&mut s, p, sid0);
        s
    };
    let after_early = {
        let mut s = after_ban.clone();
        // Early CheckCooldowns cancels and re-arms; the ban itself stays until due.
        s.cooldown_timer = Some(sid1);
        s
    };
    let (running, _guards, mut logs) =
        setup_preload(&prep, [PeerSelectionMsg::adversarial(p), PeerSelectionMsg::CheckCooldowns]);
    assert_trace_contains(
        &running,
        &[
            te_state("ps-1", &state).into(),
            te_input("ps-1", &PeerSelectionMsg::adversarial(p)).into(),
            te_is_static_peer("ps-1", p).into(),
            te_clock_suspend("ps-1").into(),
            te_peer_adversarial("ps-1", p).into(),
            te_schedule("ps-1", PeerSelectionMsg::CheckCooldowns, sid0).into(),
            te_state("ps-1", &after_ban).into(),
            te_input("ps-1", &PeerSelectionMsg::CheckCooldowns).into(),
            te_cancel_schedule("ps-1", sid0).into(),
            te_clock_suspend("ps-1").into(),
            te_random_seed("ps-1").into(),
            te_schedule("ps-1", PeerSelectionMsg::CheckCooldowns, sid1).into(),
            te_state("ps-1", &after_early).into(),
            te_clock(cooldown_instant()).into(),
            te_input("ps-1", &PeerSelectionMsg::CheckCooldowns).into(),
            te_cancel_schedule("ps-1", sid1).into(),
            te_clock_suspend("ps-1").into(),
            te_random_seed("ps-1").into(),
            te_state("ps-1", &state).into(),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["peer_selection.peer.adversarial", r#"peer="5.5.5.5:5""#])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

// ---------------------------------------------------------------------------
// Connected / Disconnected
// ---------------------------------------------------------------------------

#[test]
fn test_connected_inbound_success() {
    let prep = test_prep(&[]);
    let p = TestPrep::peer("4.4.4.4:4");
    let state = prep.state.clone();
    let msg = PeerSelectionMsg::Connected(p, conn(), ConnectionDirection::Inbound, true);
    let after = {
        let mut s = state.clone();
        s.inbound_peers.insert(p, conn());
        s
    };
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("ps-1", &state),
            te_input("ps-1", &msg),
            te_clock_suspend("ps-1"),
            te_record_advertisability("ps-1", p, true, sim_t0()),
            te_state("ps-1", &after),
        ],
    );
    logs.assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_connected_inbound_too_many() {
    let mut prep = test_prep(&[]);
    let p = TestPrep::peer("3.3.3.3:3");
    for i in 0..10u8 {
        prep.state.inbound_peers.insert(TestPrep::peer(&format!("1.1.1.{i}:1")), conn());
    }
    let state = prep.state.clone();
    let msg = PeerSelectionMsg::Connected(p, conn(), ConnectionDirection::Inbound, false);
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    // At capacity: still records advertisability, then disconnects without inserting.
    assert_trace(
        &running,
        &[
            te_state("ps-1", &state),
            te_input("ps-1", &msg),
            te_clock_suspend("ps-1"),
            te_record_advertisability("ps-1", p, false, sim_t0()),
            te_send("ps-1", "manager", ManagerMessage::Disconnect(p, ConnectionId::initial())),
            te_state("ps-1", &state),
        ],
    );
    logs.assert_and_remove(
        Level::INFO,
        &["peer_selection.peer.add_skipped", r#"peer="3.3.3.3:3""#, r#"reason="too_many_inbound""#],
    )
    .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_connected_outbound() {
    let prep = test_prep(&[]);
    let p = TestPrep::peer("2.2.2.2:2");
    let state = prep.state.clone();
    // advertisable=false: no peer-sharing schedule (see test_connected_outbound_schedules_share).
    let msg = PeerSelectionMsg::Connected(p, conn(), ConnectionDirection::Outbound, false);
    let after = {
        let mut s = state.clone();
        s.outbound_peers.insert(p, PeerState::Connected(using_conn()));
        s
    };
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace(
        &running,
        &[
            te_state("ps-1", &state),
            te_input("ps-1", &msg),
            te_clock_suspend("ps-1"),
            te_record_advertisability("ps-1", p, false, sim_t0()),
            te_send(
                "ps-1",
                "manager",
                ManagerMessage::SetLocalUse {
                    peer: p,
                    conn_id: ConnectionId::initial(),
                    local_use: LocalUse::Diffusion,
                },
            ),
            te_state("ps-1", &after),
        ],
    );
    logs.assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_connected_outbound_starts_peer_sharing() {
    let prep = test_prep(&[]);
    let p = TestPrep::peer("6.6.6.6:6");
    let state = prep.state.clone();
    let msg = PeerSelectionMsg::Connected(p, conn(), ConnectionDirection::Outbound, true);
    let after = {
        let mut s = state.clone();
        s.outbound_peers.insert(p, PeerState::Connected(using_conn()));
        s
    };
    let p_send = p;
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace_contains(
        &running,
        &[
            te_state("ps-1", &state).into(),
            te_input("ps-1", &msg).into(),
            te_record_advertisability("ps-1", p, true, sim_t0()).into(),
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
    logs.assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_share_peers_result_records_shared_peers() {
    use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

    let p = TestPrep::peer("7.7.7.7:7");
    let learned = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(9, 9, 9, 9), 3001));
    let mut prep = test_prep(&[]);
    prep.state.outbound_peers.insert(p, PeerState::Connected(using_conn()));
    let reply = PeerSelectionMsg::SharePeersResult { peer: p, peers: vec![learned] };
    let (running, _guards, mut logs) = setup(&prep, reply.clone());
    assert_trace_contains(
        &running,
        &[
            te_input("ps-1", &reply).into(),
            tm_state(
                "ps-1",
                // shared pool is in Performance; stage only regulates when added > 0
                move |s: &PeerSelection| s.outbound_peers.len() <= 3,
                "shared peer recorded",
            ),
        ],
    );
    logs.assert_and_remove(
        Level::INFO,
        &["peer_selection.sharing.received", r#"peer="7.7.7.7:7""#, "added=1", "total=1"],
    )
    .assert_and_remove(Level::INFO, &["peer_selection.peer.added", r#"peer="9.9.9.9:3001""#])
    .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_connect_failed_records_failure() {
    let mut prep = test_prep(&[]);
    let p = TestPrep::peer("5.5.5.5:5");
    prep.state.outbound_peers.insert(p, PeerState::Connecting);
    let state = prep.state.clone();
    // Empty static/snapshot: regulate finds no replacements after remove.
    let after = {
        let mut s = state.clone();
        s.outbound_peers.remove(&p);
        s
    };
    let msg = PeerSelectionMsg::ConnectFailed(p);
    let (running, _guards, mut logs) = setup(&prep, msg.clone());
    assert_trace_contains(
        &running,
        &[
            te_state("ps-1", &state).into(),
            te_input("ps-1", &msg).into(),
            te_clock_suspend("ps-1").into(),
            te_record_connection_failure("ps-1", p, sim_t0()).into(),
            te_clear_peer_availability("ps-1", p).into(),
            te_random_seed("ps-1").into(),
            te_state("ps-1", &after).into(),
        ],
    );
    logs.assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_disconnected_inbound() {
    let prep = test_prep(&[]);
    let p = TestPrep::peer("1.1.1.1:1");
    let state = prep.state.clone();
    let mut state_with_peer = state.clone();
    state_with_peer.inbound_peers.insert(p, conn());
    let msg = PeerSelectionMsg::Disconnected(p, ConnectionId::initial(), ConnectionDirection::Inbound, false);
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
            te_clear_peer_availability("ps-1", p),
            te_state("ps-1", &after),
        ],
    );
    logs.assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_disconnected_outbound_connecting_schedules_cooldown() {
    let prep = test_prep(&[]);
    let p = TestPrep::peer("0.0.0.0:0");
    let state = prep.state.clone();
    let mut state_conn = state.clone();
    state_conn.outbound_peers.insert(p, PeerState::Connecting);
    let msg = PeerSelectionMsg::Disconnected(p, ConnectionId::initial(), ConnectionDirection::Outbound, true);
    let (running, _guards, mut logs) = setup_preload(&prep, [msg.clone()]);
    // will_retry == true: no cool-down, no regulation; still clear availability claims.
    assert_trace(
        &running,
        &[
            te_state("ps-1", &state),
            te_input("ps-1", &msg),
            te_clear_peer_availability("ps-1", p),
            te_state("ps-1", &state),
        ],
    );
    logs.assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_outbound_retry_drops_dead_conn_before_reconnect() {
    let mut prep = test_prep(&[]);
    let p = TestPrep::peer("3.3.3.3:3");
    let mut ids = ConnectionId::initial();
    let id0 = ids.get_and_increment();
    let id1 = ids.get_and_increment();
    let conn0 = Connection::new(id0, true, false).with_local_use(LocalUse::Diffusion);
    let conn1 = Connection::new(id1, true, false).with_local_use(LocalUse::Diffusion);

    prep.state.outbound_peers.insert(p, PeerState::Connected(conn0));
    let start = prep.state.clone();
    let after_death = {
        let mut s = start.clone();
        s.outbound_peers.insert(p, PeerState::Connecting);
        s
    };
    let after_reconnect = {
        let mut s = start.clone();
        s.outbound_peers.insert(p, PeerState::Connected(conn1));
        s
    };

    let died = PeerSelectionMsg::Disconnected(p, id0, ConnectionDirection::Outbound, true);
    let connected = PeerSelectionMsg::Connected(p, conn1, ConnectionDirection::Outbound, false);
    let (running, _guards, mut logs) = setup_preload(&prep, [died.clone(), connected.clone()]);

    assert_trace_contains(
        &running,
        &[
            te_state("ps-1", &start).into(),
            te_input("ps-1", &died).into(),
            te_clear_peer_availability("ps-1", p).into(),
            te_state("ps-1", &after_death).into(),
            te_input("ps-1", &connected).into(),
            te_clock_suspend("ps-1").into(),
            te_record_advertisability("ps-1", p, false, sim_t0()).into(),
            te_state("ps-1", &after_reconnect).into(),
        ],
    );
    assert_trace_does_not_contain(
        &running,
        &[
            te_send("ps-1", "manager", ManagerMessage::Disconnect(p, id0)).into(),
            te_send("ps-1", "manager", ManagerMessage::AddPeer(p)).into(),
        ],
    );
    logs.assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_adversarial_static_outbound_does_not_readd_while_connected() {
    let mut prep = test_prep(&["10.0.9.1:1"]);
    let p = TestPrep::peer("10.0.9.1:1");
    prep.state.outbound_peers.insert(p, PeerState::Connected(using_conn()));
    let start = prep.state.clone();
    let sid = first_static_schedule_id();
    let after_ban = {
        let mut s = start.clone();
        s.outbound_peers.remove(&p);
        s.cooldowns.add_and_is_first(p, static_cooldown_instant());
        s.cooldown_timer = Some(sid);
        s
    };

    let (running, _guards, mut logs) = setup(&prep, PeerSelectionMsg::adversarial(p));

    // Ban must be recorded (and the live connection removed) before regulate refills.
    // After the static ban expires, CheckCooldowns may dial the peer again.
    assert_trace_contains(
        &running,
        &[
            te_state("ps-1", &start).into(),
            te_input("ps-1", &PeerSelectionMsg::adversarial(p)).into(),
            te_is_static_peer("ps-1", p).into(),
            te_send("ps-1", "manager", ManagerMessage::RemovePeer(p)).into(),
            te_peer_adversarial("ps-1", p).into(),
            te_schedule("ps-1", PeerSelectionMsg::CheckCooldowns, sid).into(),
            te_state("ps-1", &after_ban).into(),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["peer_selection.peer.adversarial", r#"peer="10.0.9.1:1""#])
        .assert_and_remove(
            Level::WARN,
            &["peer.ban", "peer_selection.peer.removed", r#"direction="outbound""#, "is_static=true"],
        )
        .assert_and_remove(Level::INFO, &["peer_selection.peer.added", r#"peer="10.0.9.1:1""#])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
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
        with_single_cooldown(&mut s, p, sid0);
        s
    };
    // Second ban at the same simulation time reuses the armed timer; heap gets another entry.
    let after_second = {
        let mut s = after_first.clone();
        s.cooldowns.add_and_is_first(p, cooldown_instant());
        s
    };
    let (running, _guards, mut logs) =
        setup_preload(&prep, [PeerSelectionMsg::adversarial(p), PeerSelectionMsg::adversarial(p)]);
    assert_trace_contains(
        &running,
        &[
            te_state("ps-1", &state).into(),
            te_input("ps-1", &PeerSelectionMsg::adversarial(p)).into(),
            te_is_static_peer("ps-1", p).into(),
            te_clock_suspend("ps-1").into(),
            te_peer_adversarial("ps-1", p).into(),
            te_schedule("ps-1", PeerSelectionMsg::CheckCooldowns, sid0).into(),
            te_state("ps-1", &after_first).into(),
            te_input("ps-1", &PeerSelectionMsg::adversarial(p)).into(),
            te_is_static_peer("ps-1", p).into(),
            te_clock_suspend("ps-1").into(),
            te_peer_adversarial("ps-1", p).into(),
            te_state("ps-1", &after_second).into(),
            te_clock(cooldown_instant()).into(),
            te_input("ps-1", &PeerSelectionMsg::CheckCooldowns).into(),
            te_cancel_schedule("ps-1", sid0).into(),
            te_clock_suspend("ps-1").into(),
            te_random_seed("ps-1").into(),
            te_state("ps-1", &state).into(),
        ],
    );
    logs.assert_and_remove(Level::DEBUG, &["peer_selection.peer.adversarial", r#"peer="11.11.11.11:11""#])
        .assert_and_remove(Level::DEBUG, &["peer_selection.peer.adversarial", r#"peer="11.11.11.11:11""#])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
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

    let (mut running, _guards, _logs) = setup_preload_until_sleeping(&prep, [PeerSelectionMsg::adversarial(p)]);

    assert!(!running.skip_to_next_wakeup(Some(intermediate)));
    assert_eq!(running.now(), intermediate);

    running.enqueue_msg(peer_selection_stage(), [PeerSelectionMsg::adversarial(p)]);
    running.run(Run::skip_and_resolve());

    assert_trace_contains(
        &running,
        &[
            te_input("ps-1", &PeerSelectionMsg::adversarial(p)).into(),
            te_schedule("ps-1", PeerSelectionMsg::CheckCooldowns, sid0).into(),
            te_clock(intermediate).into(),
            te_input("ps-1", &PeerSelectionMsg::adversarial(p)).into(),
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
    prep.state.inbound_peers.insert(p, conn());

    let (running, _guards, mut logs) = setup(&prep, PeerSelectionMsg::adversarial(p));

    // te_input + RemovePeer to Manager + final state (inbound peer count)
    assert_trace_contains(
        &running,
        &[
            te_input("ps-1", &PeerSelectionMsg::adversarial(p)).into(),
            te_send("ps-1", "manager", ManagerMessage::RemovePeer(p)).into(),
            tm_state(
                "ps-1",
                |s: &PeerSelection| !s.inbound_peers.contains_key(&p),
                "final state: inbound peer removed",
            ),
        ],
    );

    logs.assert_and_remove(Level::DEBUG, &["peer_selection.peer.adversarial", r#"peer="9.9.9.9:9""#])
        .assert_and_remove(
            Level::WARN,
            &["peer_selection.peer.removed", r#"peer="9.9.9.9:9""#, r#"direction="inbound""#],
        )
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_disconnected_outbound_connected_normal() {
    let prep = test_prep(&[]);
    let p = TestPrep::peer("8.8.8.8:8");
    let mut state = prep.state.clone();
    state.outbound_peers.insert(p, PeerState::Connected(using_conn()));

    let _after = {
        let mut s = state.clone();
        s.outbound_peers.remove(&p);
        s
    };

    let (running, _guards, mut logs) =
        setup(&prep, PeerSelectionMsg::Disconnected(p, ConnectionId::initial(), ConnectionDirection::Outbound, false));

    // te_input + final state (normal outbound Connected disconnect removes the peer, no short ban)
    assert_trace_contains(
        &running,
        &[
            te_input(
                "ps-1",
                &PeerSelectionMsg::Disconnected(p, ConnectionId::initial(), ConnectionDirection::Outbound, false),
            )
            .into(),
            tm_state(
                "ps-1",
                |s: &PeerSelection| !s.outbound_peers.contains_key(&p),
                "final state: peer removed from outbound",
            ),
        ],
    );

    logs.assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_regulate_prefers_static_before_ledger() {
    let mut prep = test_prep(&["10.0.1.1:1", "10.0.1.2:2"]).with_ledger(&["10.0.3.1:1", "10.0.3.2:2"]);

    // Trigger regulate via CheckCooldowns of a non-existent peer (cheap way to call it)
    let dummy = TestPrep::peer("10.0.9.9:9");
    prep.state.cooldowns.cooldown_until.insert(dummy, cooldown_instant());

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
                        && s.outbound_peers.contains_key(&TestPrep::peer("10.0.1.1:1"))
                        && s.outbound_peers.contains_key(&TestPrep::peer("10.0.1.2:2"))
                },
                "statics preferred; target filled including one ledger peer",
            ),
        ],
    );

    logs.assert_and_remove(Level::INFO, &["peer_selection.peer.added", r#"peer="10.0.1.2:2""#])
        .assert_and_remove(Level::INFO, &["peer_selection.peer.added", r#"peer="10.0.1.1:1""#])
        .assert_and_remove(Level::INFO, &["peer_selection.peer.added", r#"peer="10.0.3.2:2""#])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_regulate_skips_peers_in_cooldown() {
    let mut prep = test_prep(&["10.0.1.1:1", "10.0.1.2:2"]);

    // Put one static peer in cooldown
    let banned = TestPrep::peer("10.0.1.1:1");
    prep.state.cooldowns.cooldown_until.insert(banned, cooldown_instant());

    // Have a ledger candidate (seeded into Performance via TestPrep)
    let mut prep = prep.with_ledger(&["10.0.3.1:1"]);

    // Trigger regulate
    let dummy = TestPrep::peer("10.0.9.9:9");
    prep.state.cooldowns.cooldown_until.insert(dummy, cooldown_instant());

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

    logs.assert_and_remove(Level::INFO, &["peer_selection.peer.added", r#"peer="10.0.1.2:2""#])
        .assert_and_remove(Level::INFO, &["peer_selection.peer.added", r#"peer="10.0.3.1:1""#])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_disconnected_outbound_peer_also_in_inbound() {
    let mut prep = test_prep(&[]);
    let p = TestPrep::peer("7.7.7.7:7");
    prep.state.inbound_peers.insert(p, conn());
    prep.state.outbound_peers.insert(p, PeerState::Connected(using_conn()));

    let (running, _guards, mut logs) =
        setup(&prep, PeerSelectionMsg::Disconnected(p, ConnectionId::initial(), ConnectionDirection::Outbound, false));

    // te_input + final state (no RemovePeer expected for the inbound side)
    assert_trace_contains(
        &running,
        &[
            te_input(
                "ps-1",
                &PeerSelectionMsg::Disconnected(p, ConnectionId::initial(), ConnectionDirection::Outbound, false),
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

    logs.assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
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
    let first = peers[0];
    let sid = first_schedule_id();

    let (running, _guards, mut logs) = setup_preload(&prep, msgs);

    assert_trace_contains(
        &running,
        &[
            te_input("ps-1", &PeerSelectionMsg::adversarial(first)).into(),
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

    for i in 0..15 {
        logs.assert_and_remove(Level::DEBUG, &["peer_selection.peer.adversarial", &format!(r#"peer="9.9.9.{i}:9""#)]);
    }
    logs.assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_second_cooldown_does_not_schedule_again() {
    let prep = test_prep(&[]);
    let p1 = TestPrep::peer("1.1.1.1:1");
    let p2 = TestPrep::peer("2.2.2.2:2");
    let sid = first_schedule_id();
    let after_first = {
        let mut s = prep.state.clone();
        with_single_cooldown(&mut s, p1, sid);
        s
    };
    let after_second = {
        let mut s = after_first.clone();
        s.cooldowns.add_and_is_first(p2, cooldown_instant());
        s
    };

    let (running, _guards, mut logs) =
        setup_preload(&prep, [PeerSelectionMsg::adversarial(p1), PeerSelectionMsg::adversarial(p2)]);

    // Only the empty→one transition schedules; the second peer only joins the heap.
    assert_trace_contains(
        &running,
        &[
            te_state("ps-1", &prep.state).into(),
            te_input("ps-1", &PeerSelectionMsg::adversarial(p1)).into(),
            te_is_static_peer("ps-1", p1).into(),
            te_clock_suspend("ps-1").into(),
            te_peer_adversarial("ps-1", p1).into(),
            te_schedule("ps-1", PeerSelectionMsg::CheckCooldowns, sid).into(),
            te_state("ps-1", &after_first).into(),
            te_input("ps-1", &PeerSelectionMsg::adversarial(p2)).into(),
            te_is_static_peer("ps-1", p2).into(),
            te_clock_suspend("ps-1").into(),
            te_peer_adversarial("ps-1", p2).into(),
            te_state("ps-1", &after_second).into(),
            te_clock(cooldown_instant()).into(),
            te_input("ps-1", &PeerSelectionMsg::CheckCooldowns).into(),
            te_cancel_schedule("ps-1", sid).into(),
            te_clock_suspend("ps-1").into(),
            te_random_seed("ps-1").into(),
            te_state("ps-1", &prep.state).into(),
        ],
    );

    logs.assert_and_remove(Level::DEBUG, &["peer_selection.peer.adversarial", r#"peer="1.1.1.1:1""#])
        .assert_and_remove(Level::DEBUG, &["peer_selection.peer.adversarial", r#"peer="2.2.2.2:2""#])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_earlier_cooldown_reschedules_timer() {
    // Static ban = 10s, non-static = 1s. Ban static first, then a non-static peer so the shorter
    // cool-down cancels and re-arms the single timer.
    let mut prep = test_prep(&["10.0.1.1:1"]);
    let static_peer = TestPrep::peer("10.0.1.1:1");
    let other = TestPrep::peer("10.0.8.1:1");
    prep.state.outbound_peers.insert(static_peer, PeerState::Connecting);

    let sid_static = first_static_schedule_id();
    let sid_other = second_schedule_id_at(cooldown_instant());

    let (running, _guards, mut logs) =
        setup_preload(&prep, [PeerSelectionMsg::adversarial(static_peer), PeerSelectionMsg::adversarial(other)]);

    assert_trace_contains(
        &running,
        &[
            te_input("ps-1", &PeerSelectionMsg::adversarial(static_peer)).into(),
            te_clock_suspend("ps-1").into(),
            te_schedule("ps-1", PeerSelectionMsg::CheckCooldowns, sid_static).into(),
            te_input("ps-1", &PeerSelectionMsg::adversarial(other)).into(),
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

    logs.assert_and_remove(Level::DEBUG, &["peer_selection.peer.adversarial", r#"peer="10.0.1.1:1""#])
        .assert_and_remove(
            Level::WARN,
            &[
                "peer.ban",
                "peer_selection.peer.removed",
                r#"peer="10.0.1.1:1""#,
                r#"direction="outbound""#,
                "is_static=true",
            ],
        )
        .assert_and_remove(Level::DEBUG, &["peer_selection.peer.adversarial", r#"peer="10.0.8.1:1""#])
        .assert_and_remove(Level::INFO, &["peer_selection.peer.added", r#"peer="10.0.1.1:1""#, "was_banned=false"])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

// ---------------------------------------------------------------------------
// Peer-sharing responder selection (ticket 1169)
// ---------------------------------------------------------------------------

#[test]
fn test_share_candidate_pool_excludes_ledger_and_snapshot() {
    use std::collections::BTreeSet;

    use crate::performance::PeerPerformance;

    let static_p = TestPrep::peer("10.0.0.1:3001");
    let snap = TestPrep::peer("10.0.0.2:3001");
    let ledger = TestPrep::peer("10.0.0.3:3001");
    let shared = TestPrep::peer("10.0.0.4:3001");
    let mut peers = PeerPerformance::with_sources(
        BTreeSet::from([static_p]).into_iter().map(amaru_kernel::PeerCandidate::from).collect(),
        BTreeSet::from([snap]).into_iter().map(amaru_kernel::PeerCandidate::from).collect(),
        BTreeSet::from([ledger]).into_iter().map(amaru_kernel::PeerCandidate::from).collect(),
        crate::performance::PeerMix::default(),
    );
    peers.apply_ingest_shared_peers(&static_p, &["10.0.0.4:3001".parse().unwrap()]);
    // select_share uses pool of static+shared only (not ledger/snapshot).
    // Without advertisability records, ok_for_sharing is false — force via advertisability
    peers.apply_advertisability(static_p, true, sim_t0());
    peers.apply_advertisability(shared, true, sim_t0());
    let addrs = peers.apply_select_share_peers(&TestPrep::peer("9.9.9.9:1"), 10, sim_t0());
    let set: BTreeSet<_> = addrs.iter().map(|a| a.to_string()).collect();
    assert!(set.contains("10.0.0.1:3001"));
    assert!(set.contains("10.0.0.4:3001"));
    assert!(!set.contains("10.0.0.2:3001"));
    assert!(!set.contains("10.0.0.3:3001"));
}

#[test]
fn test_share_request_replies_with_selected_peers() {
    use amaru_protocols::peer_sharing::SharePeersReply;
    use amaru_pure_stage::StageRef;

    let static_a = TestPrep::peer("10.0.0.1:3001");
    let static_b = TestPrep::peer("10.0.0.2:3001");
    let requester = TestPrep::peer("10.0.0.9:3001");
    let static_a_s = static_a.to_string();
    let static_b_s = static_b.to_string();
    let mut prep = test_prep(&[static_a_s.as_str(), static_b_s.as_str()]);
    prep.state.outbound_peers.insert(static_a, PeerState::Connected(using_conn()));
    prep.state.outbound_peers.insert(static_b, PeerState::Connected(using_conn()));

    let reply_to: StageRef<SharePeersReply> = StageRef::named_for_tests("share_reply");
    let msg = PeerSelectionMsg::ShareRequest { peer: requester, amount: 10, reply_to: reply_to.clone() };
    let (running, _guards, mut logs) = setup(&prep, msg.clone());

    assert_trace_contains(
        &running,
        &[
            te_input("ps-1", &msg).into(),
            tm_send_match("ps-1", "share_reply", |r: &SharePeersReply| {
                r.peers.len() == 2
                    && r.peers.iter().any(|a| a.to_string() == "10.0.0.1:3001")
                    && r.peers.iter().any(|a| a.to_string() == "10.0.0.2:3001")
            }),
        ],
    );
    logs.assert_and_remove(
        Level::INFO,
        &["peer_selection.sharing.sent", r#"peer="10.0.0.9:3001""#, "requested=10", "count=2"],
    )
    .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_share_request_excludes_requester_and_respects_amount() {
    use amaru_protocols::peer_sharing::SharePeersReply;
    use amaru_pure_stage::StageRef;

    let peers: Vec<_> = (1..=5).map(|i| TestPrep::peer(&format!("10.0.0.{i}:3001"))).collect();
    let names: Vec<String> = peers.iter().map(ToString::to_string).collect();
    let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut prep = test_prep(&name_refs);
    for p in &peers {
        prep.state.outbound_peers.insert(*p, PeerState::Connected(using_conn()));
    }
    let requester = peers[0];
    let reply_to: StageRef<SharePeersReply> = StageRef::named_for_tests("share_reply");
    let msg = PeerSelectionMsg::ShareRequest { peer: requester, amount: 2, reply_to };
    let (running, _guards, mut logs) = setup(&prep, msg.clone());

    assert_trace_contains(
        &running,
        &[
            te_input("ps-1", &msg).into(),
            tm_send_match("ps-1", "share_reply", move |r: &SharePeersReply| {
                r.peers.len() == 2 && r.peers.iter().all(|a| a.to_string() != requester.to_string())
            }),
        ],
    );
    logs.assert_and_remove(
        Level::INFO,
        &["peer_selection.sharing.sent", r#"peer="10.0.0.1:3001""#, "requested=2", "count=2"],
    )
    .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_churn_demotes_worst_non_static_without_malus() {
    // UntilSleeping: Churn re-arms a ~3300s timer; UntilBlocked would follow it forever.
    let mut prep = test_prep(&[]);
    let a = TestPrep::peer("1.1.1.1:1");
    let b = TestPrep::peer("2.2.2.2:2");
    let c = TestPrep::peer("3.3.3.3:3");
    for p in [a, b, c] {
        prep.state.outbound_peers.insert(p, PeerState::Connected(using_conn()));
    }
    let start = prep.state.clone();
    let after = {
        let mut s = start.clone();
        s.outbound_peers.insert(a, PeerState::Connected(using_conn().with_local_use(LocalUse::Maintenance)));
        s.demoted_until.insert(a, sim_t0() + CHURN_REPROMOTE_DELAY);
        s
    };

    let (running, _guards, mut logs) = setup_preload_until_sleeping(&prep, [PeerSelectionMsg::Churn]);
    assert_trace_contains(
        &running,
        &[
            te_state("ps-1", &start).into(),
            te_input("ps-1", &PeerSelectionMsg::Churn).into(),
            te_clock_suspend("ps-1").into(),
            te_rank_peers_for_churn("ps-1", vec![a, b, c], sim_t0()).into(),
            te_is_static_peer("ps-1", a).into(),
            te_send(
                "ps-1",
                "manager",
                ManagerMessage::SetLocalUse {
                    peer: a,
                    conn_id: ConnectionId::initial(),
                    local_use: LocalUse::Maintenance,
                },
            )
            .into(),
            te_state("ps-1", &after).into(),
        ],
    );
    assert_trace_does_not_contain(&running, &[te_record_connection_failure("ps-1", a, sim_t0()).into()]);
    logs.assert_and_remove(Level::INFO, &["peer_selection.peer.demoted", r#"peer="1.1.1.1:1""#, r#"reason="churn""#])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_uninteresting_demotes_without_malus() {
    let mut prep = test_prep(&[]);
    let p = TestPrep::peer("8.8.8.8:8");
    prep.state.outbound_peers.insert(p, PeerState::Connected(using_conn()));
    let start = prep.state.clone();
    let after = {
        let mut s = start.clone();
        s.outbound_peers.insert(p, PeerState::Connected(using_conn().with_local_use(LocalUse::Maintenance)));
        s.demoted_until.insert(p, sim_t0() + UNINTERESTING_RETRY);
        s
    };
    let msg = PeerSelectionMsg::Uninteresting { peer: p, conn_id: ConnectionId::initial(), after_rollback: false };

    let (running, _guards, mut logs) = setup_preload_until_sleeping(&prep, [msg.clone()]);
    assert_trace_contains(
        &running,
        &[
            te_state("ps-1", &start).into(),
            te_input("ps-1", &msg).into(),
            te_clock_suspend("ps-1").into(),
            te_send(
                "ps-1",
                "manager",
                ManagerMessage::SetLocalUse {
                    peer: p,
                    conn_id: ConnectionId::initial(),
                    local_use: LocalUse::Maintenance,
                },
            )
            .into(),
            te_state("ps-1", &after).into(),
        ],
    );
    assert_trace_does_not_contain(&running, &[te_record_connection_failure("ps-1", p, sim_t0()).into()]);
    logs.assert_and_remove(
        Level::INFO,
        &["peer_selection.peer.demoted", r#"peer="8.8.8.8:8""#, r#"reason="uninteresting""#],
    )
    .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn test_churn_skips_static_peers() {
    // UntilSleeping: Churn re-arms a ~3300s timer; UntilBlocked would follow it forever.
    let mut prep = test_prep(&["10.0.0.1:1"]);
    let static_p = TestPrep::peer("10.0.0.1:1");
    prep.state.outbound_peers.insert(static_p, PeerState::Connected(using_conn()));
    let start = prep.state.clone();

    let (running, _guards, mut logs) = setup_preload_until_sleeping(&prep, [PeerSelectionMsg::Churn]);
    assert_trace_contains(
        &running,
        &[
            te_state("ps-1", &start).into(),
            te_input("ps-1", &PeerSelectionMsg::Churn).into(),
            te_is_static_peer("ps-1", static_p).into(),
            te_state("ps-1", &start).into(),
        ],
    );
    assert_trace_does_not_contain(
        &running,
        &[te_send(
            "ps-1",
            "manager",
            ManagerMessage::SetLocalUse {
                peer: static_p,
                conn_id: ConnectionId::initial(),
                local_use: LocalUse::Maintenance,
            },
        )
        .into()],
    );
    logs.assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}
