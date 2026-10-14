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
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use amaru_kernel::PeerCandidate;
use amaru_observability::tracing::Level;
use amaru_ouroboros::{
    CloseReason, ConnectionDirection, ConnectionId, ConnectionRecord, ObservedAt, PeerTracking, RemoteInitiators,
    RemoteProtocol,
};
use amaru_protocols::{connection::MiniProtocolUse, manager::ManagerMessage};
use amaru_pure_stage::{Effect, simulation::SimulationRunning, trace_buffer::TraceEntry};

use super::*;
use crate::{
    performance::{DialOutcome, PeerPerformance, PeerView, ResourcePerformance, ViewConnection},
    stages::{
        peer_selection::test_setup::{SIM_INITIAL_CLOCK_SECS, TestPrep, setup, sim_t0, test_prep},
        test_utils::start_in_era,
    },
};

fn peer_view(
    generation: u64,
    connections: Vec<ViewConnection>,
    connect_failures: BTreeMap<Peer, ObservedAt>,
) -> PeerView {
    PeerView { generation, connections, connect_failures, closes: BTreeMap::new(), uninteresting: Vec::new() }
}

struct ViewConn(ViewConnection);

impl ViewConn {
    fn new(peer: Peer, conn_id: ConnectionId, direction: ConnectionDirection, local_use: MiniProtocolUse) -> Self {
        Self(ViewConnection {
            peer,
            conn_id,
            direction,
            full_duplex: direction == ConnectionDirection::Inbound,
            advertisable: false,
            local_use,
            remote_initiators: RemoteInitiators::default(),
            remote_use: MiniProtocolUse::None,
        })
    }

    fn full_duplex(mut self, full_duplex: bool) -> Self {
        self.0.full_duplex = full_duplex;
        self
    }

    fn remote_use(mut self, remote_use: MiniProtocolUse) -> Self {
        self.0.remote_use = remote_use;
        self
    }

    fn build(self) -> ViewConnection {
        self.0
    }
}

fn view_conn(
    peer: Peer,
    conn_id: ConnectionId,
    direction: ConnectionDirection,
    local_use: MiniProtocolUse,
) -> ViewConnection {
    ViewConn::new(peer, conn_id, direction, local_use).build()
}

fn connected(peer: Peer, conn_id: ConnectionId, wanted: MiniProtocolUse, applied: MiniProtocolUse) -> OutboundIntent {
    let mut bearer = DesiredBearer::adopt(&view_conn(peer, conn_id, ConnectionDirection::Outbound, applied), wanted);
    bearer.local_use_sent_at = Some(sim_t0());
    OutboundIntent::Connected(bearer)
}

fn far_deadlines(state: &mut PeerSelection) {
    state.next_churn_at = Some(sim_t0() + Duration::from_secs(3600));
    state.next_sweep_at = Some(sim_t0() + Duration::from_secs(30));
}

fn suspends(running: &SimulationRunning) -> Vec<Effect> {
    running
        .trace_buffer()
        .lock()
        .iter_entries()
        .filter_map(|(_, entry)| if let TraceEntry::Suspend(effect) = entry { Some(effect) } else { None })
        .collect()
}

fn manager_sends(running: &SimulationRunning) -> Vec<ManagerMessage> {
    suspends(running)
        .into_iter()
        .filter_map(|effect| {
            if let Effect::Send { msg, .. } = effect { msg.cast::<ManagerMessage>().ok().map(|msg| *msg) } else { None }
        })
        .collect()
}

fn timeout_delays(running: &SimulationRunning) -> Vec<Duration> {
    suspends(running)
        .into_iter()
        .filter_map(|effect| if let Effect::SetTimeout { delay, .. } = effect { Some(delay) } else { None })
        .collect()
}

fn ps_state(running: &SimulationRunning) -> PeerSelection {
    let mut found = None;
    for (_, entry) in running.trace_buffer().lock().iter_entries() {
        if let TraceEntry::State { stage, state } = entry
            && stage.as_str() == "ps-1"
            && let Ok(state) = state.cast::<PeerSelection>()
        {
            found = Some(*state);
        }
    }
    found.expect("peer selection state")
}

fn counted_external<T: amaru_pure_stage::ExternalEffect>(running: &SimulationRunning) -> usize {
    suspends(running)
        .into_iter()
        .filter(|effect| matches!(effect, Effect::External { effect, .. } if effect.is::<T>()))
        .count()
}

#[test]
fn initialize_dials_static_peers_and_arms_one_timeout() {
    let prep = test_prep(&["10.0.0.1:1", "10.0.0.2:2"]);
    let (running, _guards, mut logs) = setup(&prep, PeerSelectionMsg::Initialize);
    let sends = manager_sends(&running);
    assert_eq!(sends.iter().filter(|msg| matches!(msg, ManagerMessage::AddPeer(_))).count(), 2);
    let delays = timeout_delays(&running);
    assert_eq!(delays.len(), 1);
    assert_eq!(delays[0], Duration::from_secs(1));
    logs.assert_and_remove(Level::INFO, &["peer_selection.connect_initial", "static_peers=2"])
        .assert_no_remaining_at([Level::WARN, Level::ERROR]);
}

#[test]
fn refill_after_a_close_dials_the_next_static_peer() {
    let gone = TestPrep::peer("1.1.1.1:1");
    let next = TestPrep::peer("2.2.2.2:2");
    let mut prep = test_prep(&["2.2.2.2:2"]);
    prep.state.target_upstream_peers = 1;
    prep.state
        .outbound_peers
        .insert(gone, connected(gone, ConnectionId::initial(), MiniProtocolUse::Diffusion, MiniProtocolUse::Diffusion));
    far_deadlines(&mut prep.state);
    prep.scripted_view = Some(peer_view(1, Vec::new(), BTreeMap::new()));

    let (running, _guards, _logs) = setup(&prep, PeerSelectionMsg::Tick);
    let sends = manager_sends(&running);
    assert_eq!(sends, vec![ManagerMessage::AddPeer(next)]);
    assert!(!ps_state(&running).outbound_peers.contains_key(&gone));
}

#[test]
fn connect_and_close_between_ticks_resolves_on_the_next_round() {
    let gone = TestPrep::peer("1.1.1.1:1");
    let next = TestPrep::peer("2.2.2.2:2");
    let at = ObservedAt::new(
        Duration::from_secs(crate::stages::peer_selection::test_setup::SIM_INITIAL_CLOCK_SECS),
        start_in_era().relative_time,
    );
    let conn_id = ConnectionId::initial();
    let mut peers = PeerPerformance::new();
    peers.record_connection_established(
        ConnectionRecord {
            peer: gone,
            conn_id,
            direction: ConnectionDirection::Outbound,
            full_duplex: false,
            advertisable: false,
            local_use: MiniProtocolUse::None,
            remote_initiators: RemoteInitiators::default(),
            established_at: at,
        },
        at,
    );
    peers.record_connection_closed(gone, conn_id, CloseReason::BearerEnded, at);
    let view = peers.query_peer_view(0).expect("establish and close move the generation");
    assert!(view.connections.is_empty(), "the bearer is already gone");
    assert_eq!(view.closes.get(&gone).copied(), Some(DialOutcome::Closed { at, reason: CloseReason::BearerEnded }));

    let mut prep = test_prep(&["2.2.2.2:2"]);
    prep.state.target_upstream_peers = 1;
    // The clock is still the dial start, ten seconds short of the lost-dial deadline.
    prep.state
        .outbound_peers
        .insert(gone, OutboundIntent::Dialing { since: sim_t0(), candidate: PeerCandidate::from(gone) });
    far_deadlines(&mut prep.state);
    prep.scripted_view = Some(view);

    let (running, _guards, _logs) = setup(&prep, PeerSelectionMsg::Tick);
    let state = ps_state(&running);
    assert!(!state.outbound_peers.contains_key(&gone), "the dial is still in flight");
    assert!(!state.dial_holdoff.contains_key(&PeerCandidate::from(gone)), "a close is not a connect failure");
    assert_eq!(manager_sends(&running), vec![ManagerMessage::AddPeer(next)]);
    assert!(matches!(state.outbound_peers.get(&next), Some(OutboundIntent::Dialing { .. })));
}

#[test]
fn dial_with_no_outcome_is_held_off() {
    let peer = TestPrep::peer("4.4.4.4:4");
    let mut prep = test_prep(&["4.4.4.4:4"]);
    prep.state.target_upstream_peers = 1;
    prep.state.connection_timeout = Duration::ZERO;
    let since = amaru_pure_stage::Instant::at_offset(Duration::from_secs(2), start_in_era().relative_time);
    prep.state.outbound_peers.insert(peer, OutboundIntent::Dialing { since, candidate: PeerCandidate::from(peer) });
    far_deadlines(&mut prep.state);

    let (running, _guards, _logs) = setup(&prep, PeerSelectionMsg::Tick);
    let state = ps_state(&running);
    assert!(state.outbound_peers.is_empty());
    assert!(state.dial_holdoff.contains_key(&PeerCandidate::from(peer)));
    assert!(manager_sends(&running).is_empty());
}

#[test]
fn banned_inbound_reconnect_is_disconnected() {
    let peer = TestPrep::peer("5.5.5.5:5");
    let conn_id = ConnectionId::initial();
    let mut prep = test_prep(&[]);
    prep.state.cooldowns.cooldown_until.insert(peer, sim_t0() + Duration::from_secs(60));
    far_deadlines(&mut prep.state);
    prep.scripted_view = Some(peer_view(
        1,
        vec![view_conn(peer, conn_id, ConnectionDirection::Inbound, MiniProtocolUse::None)],
        BTreeMap::new(),
    ));

    let (running, _guards, _logs) = setup(&prep, PeerSelectionMsg::Tick);
    assert_eq!(manager_sends(&running), vec![ManagerMessage::Disconnect(peer, conn_id)]);
    assert!(!ps_state(&running).inbound_peers.contains_key(&peer));
}

#[test]
fn adversarial_reports_during_a_ban_remove_the_peer_once() {
    let peer = TestPrep::peer("6.6.6.6:6");
    let prep = test_prep(&[]);
    let (running, _guards, mut logs) = crate::stages::peer_selection::test_setup::setup_preload(
        &prep,
        [PeerSelectionMsg::adversarial(peer), PeerSelectionMsg::adversarial(peer), PeerSelectionMsg::adversarial(peer)],
    );
    let removes = manager_sends(&running)
        .into_iter()
        .filter(|msg| matches!(msg, ManagerMessage::RemovePeer(p) if *p == peer))
        .count();
    assert_eq!(removes, 1);
    assert_eq!(counted_external::<crate::performance::PeerAdversarialEffect>(&running), 1);
    assert_eq!(
        ps_state(&running).cooldowns.cooldown_until.get(&peer).copied(),
        Some(sim_t0() + Duration::from_secs(1))
    );
    logs.assert_and_remove(Level::DEBUG, &["peer_selection.peer.adversarial_duplicate"])
        .assert_and_remove(Level::DEBUG, &["peer_selection.peer.adversarial_duplicate"]);
    let _ = logs;
}

#[test]
fn a_round_sends_at_most_upstream_plus_downstream_commands() {
    let mut prep = test_prep(&[]);
    prep.state.target_upstream_peers = 1;
    prep.state.target_downstream_peers = 1;
    far_deadlines(&mut prep.state);
    let mut ids = ConnectionId::initial();
    let mut connections = Vec::new();
    for n in 1..=5u16 {
        let peer = TestPrep::peer(&format!("7.0.0.{n}:7"));
        let conn_id = ids.get_and_increment();
        prep.state.cooldowns.cooldown_until.insert(peer, sim_t0() + Duration::from_secs(60));
        connections.push(view_conn(peer, conn_id, ConnectionDirection::Inbound, MiniProtocolUse::None));
    }
    let peers: Vec<Peer> = connections.iter().map(|conn| conn.peer).collect();
    for conn in &connections {
        prep.state.inbound_peers.insert(conn.peer, DesiredBearer::adopt(conn, MiniProtocolUse::None));
    }
    prep.scripted_view = Some(peer_view(1, connections, BTreeMap::new()));

    let (mut running, _guards, _logs) = setup(&prep, PeerSelectionMsg::Tick);
    let disconnects = |running: &SimulationRunning| {
        manager_sends(running).into_iter().filter(|msg| matches!(msg, ManagerMessage::Disconnect(..))).count()
    };
    assert_eq!(disconnects(&running), 2);
    let state = ps_state(&running);
    assert_eq!(state.seen_generation, 0, "a deferred disconnect keeps the view for the next tick");
    for peer in &peers[..2] {
        assert!(!state.inbound_peers.contains_key(peer), "{peer} was disconnected");
    }
    for peer in &peers[2..] {
        assert!(state.inbound_peers.contains_key(peer), "{peer} is still tracked");
    }

    // The next tick is armed from the clock after this round's effect durations, and the
    // round itself suspends on later sampled effects. One millisecond covers those tails
    // without reaching the tick after that.
    let next = running.now() + super::TICK_INTERVAL + Duration::from_millis(1);
    drive_until(&mut running, next);
    assert_eq!(disconnects(&running), 4);
    let state = ps_state(&running);
    assert_eq!(state.seen_generation, 0);
    assert!(state.inbound_peers.contains_key(&peers[4]));
    assert!(!state.inbound_peers.contains_key(&peers[2]));
    assert!(!state.inbound_peers.contains_key(&peers[3]));

    let next = running.now() + super::TICK_INTERVAL + Duration::from_millis(1);
    drive_until(&mut running, next);
    assert_eq!(disconnects(&running), 5);
    let state = ps_state(&running);
    assert!(state.inbound_peers.is_empty());
    assert_eq!(state.seen_generation, 1);
}

#[test]
fn idle_tick_skips_the_round_when_nothing_is_due() {
    let mut prep = test_prep(&["8.8.8.8:8"]);
    far_deadlines(&mut prep.state);
    let (running, _guards, _logs) = setup(&prep, PeerSelectionMsg::Tick);
    assert_eq!(counted_external::<crate::performance::SelectOutboundEffect>(&running), 0);
    assert_eq!(counted_external::<crate::effects::GenerateRandomSeed>(&running), 0);
    assert_eq!(timeout_delays(&running), vec![Duration::from_secs(1)]);
    assert!(manager_sends(&running).is_empty());
}

#[test]
fn a_due_sweep_runs_a_round_without_a_new_view() {
    let mut prep = test_prep(&[]);
    prep.state.next_churn_at = Some(sim_t0() + Duration::from_secs(3600));
    prep.state.next_sweep_at = Some(sim_t0());
    let (running, _guards, _logs) = setup(&prep, PeerSelectionMsg::Tick);
    assert_eq!(counted_external::<crate::performance::SelectOutboundEffect>(&running), 1);
    assert_eq!(counted_external::<crate::performance::EvictRecordsEffect>(&running), 0);
    assert_eq!(ps_state(&running).next_evict_at, Some(sim_t0() + EVICTION_INTERVAL));
}

#[test]
fn a_due_eviction_deadline_enqueues_one_batch() {
    let banned = TestPrep::peer("1.2.3.4:1");
    let dialing = TestPrep::peer("1.2.3.5:2");
    let mut prep = test_prep(&[]);
    prep.state.next_churn_at = Some(sim_t0() + Duration::from_secs(3600));
    prep.state.next_sweep_at = Some(sim_t0() + Duration::from_secs(30));
    prep.state.next_evict_at = Some(sim_t0());
    prep.state.cooldowns.cooldown_until.insert(banned, sim_t0() + Duration::from_secs(60));
    prep.state
        .outbound_peers
        .insert(dialing, OutboundIntent::Dialing { since: sim_t0(), candidate: PeerCandidate::from(dialing) });

    let (running, _guards, _logs) = setup(&prep, PeerSelectionMsg::Tick);
    let effects = suspends(&running)
        .into_iter()
        .filter_map(|effect| {
            let Effect::External { effect, .. } = effect else {
                return None;
            };
            effect.cast::<crate::performance::EvictRecordsEffect>().ok().map(|effect| *effect)
        })
        .collect::<Vec<_>>();
    assert_eq!(effects.len(), 1);
    assert!(effects[0].protected_peers.contains(&banned));
    assert!(effects[0].protected_peers.contains(&dialing));
    assert!(effects[0].protected_candidates.contains(&PeerCandidate::from(dialing)));
    assert_eq!(ps_state(&running).next_evict_at, Some(sim_t0() + EVICTION_INTERVAL));
}

#[test]
fn the_timeout_is_armed_for_the_earliest_deadline() {
    let mut prep = test_prep(&[]);
    far_deadlines(&mut prep.state);
    prep.state
        .dial_holdoff
        .insert(PeerCandidate::from(TestPrep::peer("9.9.9.1:9")), sim_t0() + Duration::from_millis(200));
    let (running, _guards, _logs) = setup(&prep, PeerSelectionMsg::Tick);
    assert_eq!(counted_external::<crate::performance::SelectOutboundEffect>(&running), 0);
    assert_eq!(timeout_delays(&running), vec![Duration::from_millis(200)]);
}

#[test]
fn a_failure_next_to_a_live_outbound_keeps_that_connection() {
    let peer = TestPrep::peer("9.9.9.9:9");
    let conn_id = ConnectionId::initial();
    let alias = PeerCandidate::host("relay.example".parse().unwrap(), 3001);
    let mut prep = test_prep(&[]);
    prep.state
        .outbound_peers
        .insert(peer, OutboundIntent::Dialing { since: sim_t0(), candidate: PeerCandidate::from(peer) });
    // A failed dial unbinds the name. Keeping this binding shows the failure was not applied.
    prep.state.bound.insert(alias.clone(), peer);
    far_deadlines(&mut prep.state);
    let mut connect_failures = BTreeMap::new();
    connect_failures.insert(peer, ObservedAt::new(Duration::from_secs(10), start_in_era().relative_time));
    prep.scripted_view = Some(peer_view(
        1,
        vec![view_conn(peer, conn_id, ConnectionDirection::Outbound, MiniProtocolUse::Diffusion)],
        connect_failures,
    ));

    let (running, _guards, _logs) = setup(&prep, PeerSelectionMsg::Tick);
    let state = ps_state(&running);
    match state.outbound_peers.get(&peer) {
        Some(OutboundIntent::Connected(bearer)) => {
            assert_eq!(bearer.id, conn_id);
            assert_eq!(bearer.wanted, MiniProtocolUse::Diffusion);
        }
        other => panic!("live outbound was not kept: {other:?}"),
    }
    assert!(state.bound.contains_key(&alias), "failure must not unbind the live peer");
    assert!(!manager_sends(&running).iter().any(|msg| matches!(msg, ManagerMessage::Disconnect(..))));
}

#[test]
fn churn_demotes_the_worst_non_static_peer_without_asking_per_peer() {
    let mut prep = test_prep(&[]);
    prep.state.next_churn_at = Some(sim_t0());
    prep.state.next_sweep_at = Some(sim_t0() + Duration::from_secs(30));
    let mut ids = ConnectionId::initial();
    let mut connections = Vec::new();
    for name in ["1.1.1.1:1", "2.2.2.2:2", "3.3.3.3:3"] {
        let peer = TestPrep::peer(name);
        let conn_id = ids.get_and_increment();
        prep.state
            .outbound_peers
            .insert(peer, connected(peer, conn_id, MiniProtocolUse::Diffusion, MiniProtocolUse::Diffusion));
        connections.push(view_conn(peer, conn_id, ConnectionDirection::Outbound, MiniProtocolUse::Diffusion));
    }
    prep.scripted_view = Some(peer_view(1, connections, BTreeMap::new()));

    let (running, _guards, _logs) = setup(&prep, PeerSelectionMsg::Tick);
    let demoted: Vec<_> = manager_sends(&running)
        .into_iter()
        .filter_map(|msg| {
            if let ManagerMessage::SetLocalUse { peer, local_use: MiniProtocolUse::Maintenance, .. } = msg {
                Some(peer)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(demoted, vec![TestPrep::peer("1.1.1.1:1")]);
    assert_eq!(counted_external::<crate::performance::IsStaticPeerEffect>(&running), 0);
}

#[test]
fn a_share_result_with_new_candidates_dials_on_the_next_round() {
    let learned = TestPrep::peer("8.8.8.8:8");
    let donor = TestPrep::peer("9.9.9.9:9");
    let mut prep = test_prep(&[]);
    prep.state.target_upstream_peers = 1;
    prep.peer_mix = "shared~1".parse().expect("shared-only mix");
    far_deadlines(&mut prep.state);
    prep.learned_share = Some((donor, std::net::SocketAddr::from(learned)));

    let (running, _guards, _logs) = setup(&prep, PeerSelectionMsg::Tick);
    assert_eq!(manager_sends(&running), vec![ManagerMessage::AddPeer(learned)]);
}

fn set_local_uses(running: &SimulationRunning) -> Vec<(Peer, MiniProtocolUse)> {
    manager_sends(running)
        .into_iter()
        .filter_map(|msg| {
            if let ManagerMessage::SetLocalUse { peer, local_use, .. } = msg { Some((peer, local_use)) } else { None }
        })
        .collect()
}

fn wanted(state: &PeerSelection, peer: Peer) -> MiniProtocolUse {
    match state.outbound_peers.get(&peer) {
        Some(OutboundIntent::Connected(bearer)) => bearer.wanted,
        other => panic!("expected a connected bearer for {peer}, got {other:?}"),
    }
}

fn drive_until(running: &mut SimulationRunning, until: amaru_pure_stage::Instant) {
    use amaru_pure_stage::simulation::{Externals, Run, TimeAdvance};
    running.run(Run { time: TimeAdvance::Until(until), externals: Externals::Resolve });
}

#[test]
fn an_uninteresting_mark_demotes_on_the_next_round_and_promotes_when_due() {
    let peer = TestPrep::peer("1.2.3.4:4");
    let conn_id = ConnectionId::initial();
    let mut prep = test_prep(&["1.2.3.4:4"]);
    prep.state.target_upstream_peers = 1;
    far_deadlines(&mut prep.state);
    prep.established = Some((peer, conn_id));
    prep.uninteresting = Some((peer, conn_id, false));

    let (mut running, _guards, _logs) = setup(&prep, PeerSelectionMsg::Tick);
    let state = ps_state(&running);
    assert_eq!(wanted(&state, peer), MiniProtocolUse::Maintenance);
    assert_eq!(state.demoted_until.get(&peer).copied(), Some(sim_t0() + UNINTERESTING_RETRY));
    assert_eq!(set_local_uses(&running), vec![(peer, MiniProtocolUse::Maintenance)]);

    let deadline = sim_t0() + UNINTERESTING_RETRY;
    drive_until(&mut running, deadline);
    let state = ps_state(&running);
    assert_eq!(wanted(&state, peer), MiniProtocolUse::Diffusion);
    assert!(!state.demoted_until.contains_key(&peer));
    let uses = set_local_uses(&running);
    assert_eq!(uses.first().copied(), Some((peer, MiniProtocolUse::Maintenance)));
    assert_eq!(uses.last().copied(), Some((peer, MiniProtocolUse::Diffusion)));
    assert_eq!(uses.iter().filter(|(_, local_use)| *local_use == MiniProtocolUse::Diffusion).count(), 1);
}

#[test]
fn a_repeated_uninteresting_mark_keeps_the_demotion_deadline() {
    let peer = TestPrep::peer("1.2.3.5:5");
    let conn_id = ConnectionId::initial();
    let mut prep = test_prep(&["1.2.3.5:5"]);
    prep.state.target_upstream_peers = 1;
    far_deadlines(&mut prep.state);
    prep.established = Some((peer, conn_id));
    prep.uninteresting = Some((peer, conn_id, false));

    let (mut running, _guards, _logs) = setup(&prep, PeerSelectionMsg::Tick);
    let deadline = ps_state(&running).demoted_until.get(&peer).copied();
    assert_eq!(deadline, Some(sim_t0() + UNINTERESTING_RETRY));

    let performance = running.resources().get::<ResourcePerformance>().expect("performance").clone();
    performance.record_uninteresting(
        peer,
        conn_id,
        true,
        ObservedAt::new(Duration::from_secs(SIM_INITIAL_CLOCK_SECS), start_in_era().relative_time),
    );
    drive_until(&mut running, sim_t0() + Duration::from_secs(1));

    let state = ps_state(&running);
    assert_eq!(wanted(&state, peer), MiniProtocolUse::Maintenance);
    assert_eq!(state.demoted_until.get(&peer).copied(), deadline);
    assert_eq!(set_local_uses(&running), vec![(peer, MiniProtocolUse::Maintenance)]);
}

#[test]
fn an_uninteresting_mark_after_rollback_demotes_for_180s() {
    let peer = TestPrep::peer("1.2.3.6:6");
    let conn_id = ConnectionId::initial();
    let mut prep = test_prep(&["1.2.3.6:6"]);
    prep.state.target_upstream_peers = 1;
    far_deadlines(&mut prep.state);
    prep.established = Some((peer, conn_id));
    prep.uninteresting = Some((peer, conn_id, true));

    let (running, _guards, _logs) = setup(&prep, PeerSelectionMsg::Tick);
    let state = ps_state(&running);
    assert_eq!(wanted(&state, peer), MiniProtocolUse::Maintenance);
    assert_eq!(state.demoted_until.get(&peer).copied(), Some(sim_t0() + UNINTERESTING_RETRY_AFTER_ROLLBACK));
}

#[test]
fn an_uninteresting_mark_for_a_gone_peer_does_nothing() {
    let peer = TestPrep::peer("5.5.5.5:5");
    let mut prep = test_prep(&[]);
    prep.state.target_upstream_peers = 1;
    far_deadlines(&mut prep.state);
    prep.uninteresting = Some((peer, ConnectionId::initial(), false));

    let (running, _guards, _logs) = setup(&prep, PeerSelectionMsg::Tick);
    let state = ps_state(&running);
    assert_eq!(state.seen_generation, 0, "a mark with no bearer does not move the generation");
    assert!(!state.demoted_until.contains_key(&peer));
    assert!(!state.outbound_peers.contains_key(&peer));
    assert!(set_local_uses(&running).is_empty());
}

#[test]
fn a_ledger_candidate_write_dials_on_the_next_round() {
    let peer = TestPrep::peer("8.8.8.8:8");
    let mut prep = test_prep(&[]);
    prep.state.target_upstream_peers = 1;
    prep.peer_mix = "ledger~1".parse().expect("ledger-only mix");
    far_deadlines(&mut prep.state);
    prep.ledger_write = Some(BTreeSet::from([PeerCandidate::from(peer)]));

    let (running, _guards, _logs) = setup(&prep, PeerSelectionMsg::Tick);
    assert_eq!(manager_sends(&running), vec![ManagerMessage::AddPeer(peer)]);
}

#[test]
fn a_started_dial_does_not_set_holdoff() {
    let peer = TestPrep::peer("10.0.0.1:1");
    let mut prep = test_prep(&["10.0.0.1:1"]);
    prep.state.target_upstream_peers = 1;
    let (running, _guards, _logs) = setup(&prep, PeerSelectionMsg::Initialize);
    let state = ps_state(&running);
    assert!(matches!(state.outbound_peers.get(&peer), Some(OutboundIntent::Dialing { .. })));
    assert!(state.dial_holdoff.is_empty(), "starting a dial is not a connect failure");
}

#[test]
fn a_repeated_close_leaves_the_refill_dial_in_flight() {
    let peer = TestPrep::peer("1.1.1.1:1");
    let at = ObservedAt::new(Duration::from_secs(SIM_INITIAL_CLOCK_SECS), start_in_era().relative_time);
    let mut closes = BTreeMap::new();
    closes.insert(peer, DialOutcome::Closed { at, reason: CloseReason::BearerEnded });
    let view = PeerView {
        generation: 1,
        connections: Vec::new(),
        connect_failures: BTreeMap::new(),
        closes,
        uninteresting: Vec::new(),
    };

    let mut prep = test_prep(&["1.1.1.1:1"]);
    prep.state.target_upstream_peers = 1;
    prep.state
        .outbound_peers
        .insert(peer, OutboundIntent::Dialing { since: sim_t0(), candidate: PeerCandidate::from(peer) });
    far_deadlines(&mut prep.state);
    prep.scripted_view = Some(view);

    let (mut running, _guards, _logs) = setup(&prep, PeerSelectionMsg::Tick);
    let state = ps_state(&running);
    assert!(
        matches!(state.outbound_peers.get(&peer), Some(OutboundIntent::Dialing { since, .. }) if *since == sim_t0()),
        "the same round refills the closed peer: {:?}",
        state.outbound_peers.get(&peer)
    );
    assert!(state.dial_holdoff.is_empty(), "a close clears hold-off and a dial does not set it");
    assert_eq!(state.applied_close.get(&peer).copied(), Some(sim_t0()));
    assert_eq!(manager_sends(&running), vec![ManagerMessage::AddPeer(peer)]);

    // The scripted view is returned on every tick, which is what a later generation bump does
    // when it repeats the same close.
    drive_until(&mut running, sim_t0() + Duration::from_secs(1));
    let state = ps_state(&running);
    assert!(
        matches!(state.outbound_peers.get(&peer), Some(OutboundIntent::Dialing { since, .. }) if *since == sim_t0()),
        "a later view that repeats the same close must not drop the refill: {:?}",
        state.outbound_peers.get(&peer)
    );
    assert_eq!(manager_sends(&running), vec![ManagerMessage::AddPeer(peer)]);
    assert!(state.dial_holdoff.is_empty());
}

#[test]
fn a_due_demotion_lifts_while_upstream_slots_stay_full() {
    let keeper = TestPrep::peer("1.1.1.1:1");
    let refill = TestPrep::peer("2.2.2.2:2");
    let demoted = TestPrep::peer("3.3.3.3:3");
    let mut ids = ConnectionId::initial();
    let keeper_id = ids.get_and_increment();
    let refill_id = ids.get_and_increment();
    let demoted_id = ids.get_and_increment();

    let mut prep = test_prep(&[]);
    prep.state.target_upstream_peers = 2;
    prep.state.next_churn_at = Some(sim_t0() + Duration::from_secs(3600));
    prep.state.next_sweep_at = Some(sim_t0() + Duration::from_secs(3600));
    prep.state
        .outbound_peers
        .insert(keeper, connected(keeper, keeper_id, MiniProtocolUse::Diffusion, MiniProtocolUse::Diffusion));
    prep.state
        .outbound_peers
        .insert(refill, connected(refill, refill_id, MiniProtocolUse::Diffusion, MiniProtocolUse::Diffusion));
    prep.state
        .outbound_peers
        .insert(demoted, connected(demoted, demoted_id, MiniProtocolUse::Maintenance, MiniProtocolUse::Maintenance));
    let deadline = sim_t0() + Duration::from_secs(1);
    prep.state.demoted_until.insert(demoted, deadline);

    let (mut running, _guards, _logs) = setup(&prep, PeerSelectionMsg::Tick);
    let state = ps_state(&running);
    assert_eq!(state.demoted_until.get(&demoted).copied(), Some(deadline), "a future deadline stays");
    assert_eq!(wanted(&state, demoted), MiniProtocolUse::Maintenance);

    drive_until(&mut running, deadline);
    let state = ps_state(&running);
    assert!(!state.demoted_until.contains_key(&demoted));
    assert_eq!(wanted(&state, demoted), MiniProtocolUse::Maintenance);
    assert!(set_local_uses(&running).is_empty());
    let sweep_after_deadline = state.next_sweep_at;

    drive_until(&mut running, deadline + Duration::from_secs(1));
    let state = ps_state(&running);
    assert!(!state.demoted_until.contains_key(&demoted));
    assert_eq!(wanted(&state, demoted), MiniProtocolUse::Maintenance);
    assert!(set_local_uses(&running).is_empty());
    assert_eq!(state.next_sweep_at, sweep_after_deadline, "the following tick must not run another full round");
    assert_eq!(timeout_delays(&running).last().copied(), Some(Duration::from_secs(1)));
}

#[test]
fn an_inbound_disconnect_clears_a_future_demotion_deadline() {
    let peer = TestPrep::peer("3.3.3.3:3");
    let conn_id = ConnectionId::initial();
    let mut prep = test_prep(&[]);
    far_deadlines(&mut prep.state);
    let mut bearer = DesiredBearer::adopt(
        &view_conn(peer, conn_id, ConnectionDirection::Inbound, MiniProtocolUse::Maintenance),
        MiniProtocolUse::Maintenance,
    );
    bearer.local_use_sent_at = Some(sim_t0());
    prep.state.inbound_peers.insert(peer, bearer);
    prep.state.demoted_until.insert(peer, sim_t0() + Duration::from_secs(120));
    prep.scripted_view = Some(peer_view(1, Vec::new(), BTreeMap::new()));

    let (running, _guards, _logs) = setup(&prep, PeerSelectionMsg::Tick);
    let state = ps_state(&running);
    assert!(!state.inbound_peers.contains_key(&peer));
    assert!(!state.demoted_until.contains_key(&peer));
}

#[test]
fn a_cooling_inbound_disconnect_clears_a_future_demotion_deadline() {
    let peer = TestPrep::peer("3.3.3.4:4");
    let conn_id = ConnectionId::initial();
    let mut prep = test_prep(&[]);
    far_deadlines(&mut prep.state);
    prep.state.cooldowns.cooldown_until.insert(peer, sim_t0() + Duration::from_secs(60));
    let mut bearer = DesiredBearer::adopt(
        &view_conn(peer, conn_id, ConnectionDirection::Inbound, MiniProtocolUse::Maintenance),
        MiniProtocolUse::Maintenance,
    );
    bearer.local_use_sent_at = Some(sim_t0());
    prep.state.inbound_peers.insert(peer, bearer);
    prep.state.demoted_until.insert(peer, sim_t0() + Duration::from_secs(120));
    prep.scripted_view = Some(peer_view(
        1,
        vec![view_conn(peer, conn_id, ConnectionDirection::Inbound, MiniProtocolUse::Maintenance)],
        BTreeMap::new(),
    ));

    let (running, _guards, _logs) = setup(&prep, PeerSelectionMsg::Tick);
    assert_eq!(manager_sends(&running), vec![ManagerMessage::Disconnect(peer, conn_id)]);
    let state = ps_state(&running);
    assert!(!state.inbound_peers.contains_key(&peer));
    assert!(!state.demoted_until.contains_key(&peer));
}

fn disconnects(running: &SimulationRunning) -> Vec<(Peer, ConnectionId)> {
    manager_sends(running)
        .into_iter()
        .filter_map(
            |msg| if let ManagerMessage::Disconnect(peer, conn_id) = msg { Some((peer, conn_id)) } else { None },
        )
        .collect()
}

fn observed_now() -> ObservedAt {
    ObservedAt::new(Duration::from_secs(SIM_INITIAL_CLOCK_SECS), start_in_era().relative_time)
}

fn cold_inbound(peer: Peer, conn_id: ConnectionId, full_duplex: bool) -> ConnectionRecord {
    ConnectionRecord {
        peer,
        conn_id,
        direction: ConnectionDirection::Inbound,
        full_duplex,
        advertisable: false,
        local_use: MiniProtocolUse::None,
        remote_initiators: RemoteInitiators::default(),
        established_at: observed_now(),
    }
}

#[test]
fn a_new_non_duplex_cold_bearer_waits_out_the_startup_dwell() {
    let peer = TestPrep::peer("5.5.5.6:6");
    let conn_id = ConnectionId::initial();
    let mut prep = test_prep(&[]);
    prep.state.target_upstream_peers = 0;
    far_deadlines(&mut prep.state);
    prep.established_connections.push(cold_inbound(peer, conn_id, false));

    let (mut running, _guards, _logs) = setup(&prep, PeerSelectionMsg::Tick);
    let deadline = sim_t0() + REMOTE_COLD_DWELL;
    let state = ps_state(&running);
    let bearer = state.inbound_peers.get(&peer).expect("bearer kept");
    assert!(!bearer.remote_was_hot);
    assert_eq!(bearer.remote_cold_until, Some(deadline));
    assert!(disconnects(&running).is_empty());

    drive_until(&mut running, deadline);
    assert_eq!(disconnects(&running), vec![(peer, conn_id)]);
    assert!(!ps_state(&running).inbound_peers.contains_key(&peer));
}

#[test]
fn a_non_duplex_bearer_that_has_been_hot_closes_when_it_goes_cold() {
    let peer = TestPrep::peer("5.5.5.6:16");
    let conn_id = ConnectionId::initial();
    let mut prep = test_prep(&[]);
    prep.state.target_upstream_peers = 0;
    far_deadlines(&mut prep.state);
    let mut bearer = DesiredBearer::adopt(
        &ViewConn::new(peer, conn_id, ConnectionDirection::Inbound, MiniProtocolUse::None).full_duplex(false).build(),
        MiniProtocolUse::None,
    );
    bearer.remote_was_hot = true;
    prep.state.inbound_peers.insert(peer, bearer);
    prep.scripted_view = Some(peer_view(
        1,
        vec![
            ViewConn::new(peer, conn_id, ConnectionDirection::Inbound, MiniProtocolUse::None)
                .full_duplex(false)
                .build(),
        ],
        BTreeMap::new(),
    ));

    let (running, _guards, _logs) = setup(&prep, PeerSelectionMsg::Tick);
    assert_eq!(disconnects(&running), vec![(peer, conn_id)]);
    assert!(!ps_state(&running).inbound_peers.contains_key(&peer));
}

#[test]
fn a_non_duplex_bearer_with_remote_maintenance_is_kept() {
    let peer = TestPrep::peer("5.5.5.7:7");
    let conn_id = ConnectionId::initial();
    let mut prep = test_prep(&[]);
    far_deadlines(&mut prep.state);
    prep.scripted_view = Some(peer_view(
        1,
        vec![
            ViewConn::new(peer, conn_id, ConnectionDirection::Inbound, MiniProtocolUse::None)
                .full_duplex(false)
                .remote_use(MiniProtocolUse::Maintenance)
                .build(),
        ],
        BTreeMap::new(),
    ));

    let (running, _guards, _logs) = setup(&prep, PeerSelectionMsg::Tick);
    assert!(disconnects(&running).is_empty());
    let state = ps_state(&running);
    let bearer = state.inbound_peers.get(&peer).expect("bearer kept");
    assert_eq!(bearer.wanted, MiniProtocolUse::None);
    assert_eq!(bearer.remote_use, MiniProtocolUse::Maintenance);
    assert!(bearer.remote_cold_until.is_none());
}

#[test]
fn a_duplex_cold_bearer_stays_until_the_dwell_elapses() {
    let peer = TestPrep::peer("5.5.5.8:8");
    let conn_id = ConnectionId::initial();
    let mut prep = test_prep(&[]);
    prep.state.target_upstream_peers = 0;
    far_deadlines(&mut prep.state);
    prep.established_connections.push(cold_inbound(peer, conn_id, true));

    let (mut running, _guards, _logs) = setup(&prep, PeerSelectionMsg::Tick);
    let deadline = sim_t0() + REMOTE_COLD_DWELL;
    let state = ps_state(&running);
    let bearer = state.inbound_peers.get(&peer).expect("duplex bearer kept");
    assert_eq!(bearer.wanted, MiniProtocolUse::None);
    assert_eq!(bearer.remote_use, MiniProtocolUse::None);
    assert_eq!(bearer.remote_cold_until, Some(deadline));
    assert!(disconnects(&running).is_empty());

    drive_until(&mut running, deadline - Duration::from_secs(1));
    let state = ps_state(&running);
    assert_eq!(state.inbound_peers.get(&peer).and_then(|bearer| bearer.remote_cold_until), Some(deadline));
    assert!(disconnects(&running).is_empty());

    drive_until(&mut running, deadline);
    assert_eq!(disconnects(&running), vec![(peer, conn_id)]);
    assert!(!ps_state(&running).inbound_peers.contains_key(&peer));
}

#[test]
fn a_duplex_cold_deadline_is_cancelled_when_remote_use_rises() {
    let peer = TestPrep::peer("5.5.5.9:9");
    let conn_id = ConnectionId::initial();
    let mut prep = test_prep(&[]);
    prep.state.target_upstream_peers = 0;
    far_deadlines(&mut prep.state);
    prep.established_connections.push(cold_inbound(peer, conn_id, true));

    let (mut running, _guards, _logs) = setup(&prep, PeerSelectionMsg::Tick);
    assert_eq!(
        ps_state(&running).inbound_peers.get(&peer).and_then(|bearer| bearer.remote_cold_until),
        Some(sim_t0() + REMOTE_COLD_DWELL)
    );

    let performance = running.resources().get::<ResourcePerformance>().expect("performance").clone();
    prep.rt.block_on(performance.record_remote_use(peer, conn_id, RemoteProtocol::KeepAlive, true, observed_now()));
    drive_until(&mut running, sim_t0() + Duration::from_secs(1));

    assert!(disconnects(&running).is_empty());
    let state = ps_state(&running);
    let bearer = state.inbound_peers.get(&peer).expect("bearer kept");
    assert_eq!(bearer.remote_use, MiniProtocolUse::Maintenance);
    assert!(bearer.remote_cold_until.is_none());
}

#[test]
fn a_cold_deadline_wakes_a_round_before_the_sweep() {
    let peer = TestPrep::peer("5.5.5.10:10");
    let conn_id = ConnectionId::initial();
    let mut prep = test_prep(&[]);
    prep.state.target_upstream_peers = 0;
    far_deadlines(&mut prep.state);
    let mut bearer = DesiredBearer::adopt(
        &ViewConn::new(peer, conn_id, ConnectionDirection::Inbound, MiniProtocolUse::None).build(),
        MiniProtocolUse::None,
    );
    let deadline = sim_t0() + Duration::from_secs(5);
    bearer.remote_cold_until = Some(deadline);
    prep.state.inbound_peers.insert(peer, bearer);

    let (mut running, _guards, _logs) = setup(&prep, PeerSelectionMsg::Tick);
    assert!(disconnects(&running).is_empty(), "the dwell is still ahead of the first tick");
    assert!(ps_state(&running).inbound_peers.contains_key(&peer));

    drive_until(&mut running, deadline - Duration::from_secs(1));
    assert!(disconnects(&running).is_empty());

    drive_until(&mut running, deadline);
    assert_eq!(disconnects(&running), vec![(peer, conn_id)]);
    assert!(!ps_state(&running).inbound_peers.contains_key(&peer));
}
