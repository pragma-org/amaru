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

#![expect(clippy::wildcard_enum_match_arm)]

use std::sync::Arc;

use amaru_kernel::{NetworkMagic, PREPROD_ERA_HISTORY, Peer};
use amaru_ouroboros::{
    CloseReason, ConnectionDirection, ConnectionId, ConnectionRecord, MiniProtocolUse, ObservedAt,
    PeerTrackingResource, RemoteInitiators,
};
use amaru_pure_stage::{
    StageGraph, StageRef,
    simulation::{Externals, Run, SimulationBuilder, TimeAdvance, running::OverrideResult},
};
use tokio::runtime::Runtime;

use super::{Manager, ManagerConfig, ManagerMessage, OutboundState, stage};
use crate::{
    connection::ConnectionMessage,
    network_effects::{CloseEffect, ConnectEffect, ConnectError},
    peer_tracking::{InMemoryPeerTracking, observed_at},
    protocol::Role,
};

struct Harness {
    _rt: Runtime,
    manager: amaru_pure_stage::stage_ref::StageStateRef<ManagerMessage, Manager>,
    running: amaru_pure_stage::simulation::SimulationRunning,
    connection: StageRef<ConnectionMessage>,
    connection_rx: amaru_pure_stage::Receiver<ConnectionMessage>,
    extra: StageRef<ConnectionMessage>,
    extra_rx: amaru_pure_stage::Receiver<ConnectionMessage>,
    recorder: Arc<InMemoryPeerTracking>,
    _guards: amaru_pure_stage::DeserializerGuards,
}

fn harness() -> Harness {
    harness_with(ManagerConfig::default())
}

fn harness_with(config: ManagerConfig) -> Harness {
    let _guards = {
        let mut guards = super::register_deserializers();
        guards.extend(crate::peer_tracking_effects::register_deserializers());
        guards.extend(crate::network_effects::register_deserializers());
        guards
    };
    let recorder = Arc::new(InMemoryPeerTracking::new());
    let mut network = SimulationBuilder::default().with_mailbox_size(32);
    network.resources().put::<PeerTrackingResource>(recorder.clone());
    let (connection, connection_rx) = network.output("connection", 8);
    let (extra, extra_rx) = network.output("extra-connection", 8);
    let manager = network.stage("manager", stage);
    let manager = network.wire_up(
        manager,
        Manager::new(
            NetworkMagic::PREPROD,
            config,
            Arc::new(PREPROD_ERA_HISTORY.clone()),
            StageRef::blackhole(),
            StageRef::blackhole(),
        ),
    );
    let rt = Runtime::new().expect("runtime");
    let mut running = network.run(rt.handle());
    running.override_external_effect::<CloseEffect>(usize::MAX, |_| OverrideResult::handled(Ok(())));
    Harness { _rt: rt, manager, running, connection, connection_rx, extra, extra_rx, recorder, _guards }
}

fn handshake(
    peer: Peer,
    stage: StageRef<ConnectionMessage>,
    conn_id: ConnectionId,
    role: Role,
    advertisable: bool,
) -> ManagerMessage {
    ManagerMessage::HandshakeComplete {
        peer,
        stage,
        conn_id,
        role,
        full_duplex: false,
        advertisable,
        caller: StageRef::blackhole(),
    }
}

fn record_for(
    peer: Peer,
    conn_id: ConnectionId,
    direction: ConnectionDirection,
    advertisable: bool,
    local_use: MiniProtocolUse,
    at: ObservedAt,
) -> ConnectionRecord {
    ConnectionRecord {
        peer,
        conn_id,
        direction,
        full_duplex: false,
        advertisable,
        local_use,
        remote_initiators: RemoteInitiators::default(),
        established_at: at,
    }
}

fn ids() -> (ConnectionId, ConnectionId) {
    let mut id = ConnectionId::initial();
    let first = id.get_and_increment();
    (first, id.get_and_increment())
}

fn id_n(n: usize) -> Vec<ConnectionId> {
    let mut id = ConnectionId::initial();
    (0..n).map(|_| id.get_and_increment()).collect()
}

fn virtual_children(sim: &mut Harness) {
    sim.running.use_virtual_child_stages(true);
}

fn connection_ok(peer: Peer, attempt: u64, conn_id: ConnectionId) -> ManagerMessage {
    ManagerMessage::ConnectionResult { peer, attempt, result: Ok(conn_id) }
}

fn connection_err(peer: Peer, attempt: u64) -> ManagerMessage {
    ManagerMessage::ConnectionResult { peer, attempt, result: Err(ConnectError::new(peer, "refused")) }
}

#[test]
fn outbound_handshake_is_recorded_and_still_notified() {
    let mut sim = harness();
    virtual_children(&mut sim);
    let peer = Peer::for_test(4101);
    let (conn_id, _) = ids();
    sim.running.enqueue_msg(
        &sim.manager,
        [
            ManagerMessage::AddPeer(peer),
            connection_ok(peer, 1, conn_id),
            handshake(peer, sim.connection.clone(), conn_id, Role::Initiator, true),
        ],
    );
    sim.running.run(Run::skip_and_resolve()).assert_idle();

    let at = observed_at(sim.running.now());
    assert_eq!(
        sim.recorder.established(),
        vec![(record_for(peer, conn_id, ConnectionDirection::Outbound, true, MiniProtocolUse::Diffusion, at), at)]
    );
    assert!(sim.recorder.closed().is_empty());
    assert!(sim.recorder.connect_failures().is_empty());
}

#[test]
fn inbound_handshake_starts_at_no_local_use() {
    let mut sim = harness();
    virtual_children(&mut sim);
    let peer = Peer::for_test(4102);
    let (conn_id, _) = ids();
    sim.running.enqueue_msg(
        &sim.manager,
        [
            ManagerMessage::Accepted(peer, conn_id),
            handshake(peer, sim.connection.clone(), conn_id, Role::Responder, false),
        ],
    );
    sim.running.run(Run::skip_and_resolve()).assert_idle();

    let at = observed_at(sim.running.now());
    assert_eq!(
        sim.recorder.established(),
        vec![(record_for(peer, conn_id, ConnectionDirection::Inbound, false, MiniProtocolUse::None, at), at)]
    );
}

#[test]
fn a_new_inbound_replaces_the_handshake_still_in_flight() {
    let mut sim = harness();
    virtual_children(&mut sim);
    let peer = Peer::for_test(4103);
    let (conn_id, extra_id) = ids();
    sim.running.enqueue_msg(
        &sim.manager,
        [
            ManagerMessage::Accepted(peer, conn_id),
            ManagerMessage::Accepted(peer, extra_id),
            handshake(peer, sim.connection.clone(), conn_id, Role::Responder, false),
            handshake(peer, sim.extra.clone(), extra_id, Role::Responder, false),
        ],
    );
    sim.running.run(Run::skip_and_resolve()).assert_idle();

    assert_eq!(sim.recorder.established().len(), 1);
    assert_eq!(sim.recorder.established()[0].0.conn_id, extra_id);
    assert!(sim.recorder.closed().is_empty(), "the replaced handshake was never established");
    assert!(sim.connection_rx.drain().next().is_none());
    assert!(sim.extra_rx.drain().next().is_none());
    let state = sim.running.get_state(&sim.manager).expect("manager idle");
    assert_eq!(state.peers[&peer].inbound, Some(extra_id));
}

#[test]
fn remove_peer_closes_each_bearer_and_still_notifies() {
    let mut sim = harness();
    virtual_children(&mut sim);
    let peer = Peer::for_test(4105);
    let (inbound, outbound) = ids();
    sim.running.enqueue_msg(
        &sim.manager,
        [
            ManagerMessage::Accepted(peer, inbound),
            ManagerMessage::AddPeer(peer),
            connection_ok(peer, 1, outbound),
            handshake(peer, sim.connection.clone(), inbound, Role::Responder, false),
            handshake(peer, sim.extra.clone(), outbound, Role::Initiator, true),
            ManagerMessage::RemovePeer(peer),
        ],
    );
    sim.running.run(Run::skip_and_resolve()).assert_idle();

    let at = observed_at(sim.running.now());
    assert_eq!(
        sim.recorder.closed(),
        vec![(peer, inbound, CloseReason::LocalDisconnect, at), (peer, outbound, CloseReason::LocalDisconnect, at),]
    );
}

#[test]
fn bearer_death_closes_with_bearer_ended_and_a_later_death_does_not_write_again() {
    let mut sim = harness();
    virtual_children(&mut sim);
    let peer = Peer::for_test(4106);
    let (conn_id, _) = ids();
    sim.running.enqueue_msg(
        &sim.manager,
        [
            ManagerMessage::AddPeer(peer),
            connection_ok(peer, 1, conn_id),
            handshake(peer, sim.connection.clone(), conn_id, Role::Initiator, true),
            ManagerMessage::ConnectionDied(peer, conn_id, Role::Initiator),
            ManagerMessage::ConnectionDied(peer, conn_id, Role::Initiator),
        ],
    );
    sim.running.run(Run::skip_and_resolve()).assert_idle();

    let at = observed_at(sim.running.now());
    assert_eq!(sim.recorder.closed(), vec![(peer, conn_id, CloseReason::BearerEnded, at)]);
    assert!(sim.recorder.connect_failures().is_empty());
}

#[test]
fn failed_attempt_is_recorded_once_and_still_notified() {
    let mut sim = harness();
    let peer = Peer::for_test(4107);
    sim.running.override_external_effect::<ConnectEffect>(usize::MAX, move |_| {
        OverrideResult::handled(Err(ConnectError::new(peer, "refused")))
    });
    sim.running.enqueue_msg(&sim.manager, [ManagerMessage::AddPeer(peer)]);
    sim.running.run(Run::skip_and_resolve()).assert_idle();

    let at = observed_at(sim.running.now());
    assert_eq!(sim.recorder.connect_failures(), vec![(peer, at)]);
    assert!(sim.recorder.established().is_empty());
}

#[test]
fn initiator_death_before_handshake_is_recorded_once() {
    let mut sim = harness();
    virtual_children(&mut sim);
    let peer = Peer::for_test(4108);
    let (conn_id, unknown) = ids();
    sim.running.enqueue_msg(
        &sim.manager,
        [
            ManagerMessage::AddPeer(peer),
            connection_ok(peer, 1, conn_id),
            ManagerMessage::ConnectionDied(peer, unknown, Role::Initiator),
            ManagerMessage::ConnectionDied(peer, conn_id, Role::Initiator),
        ],
    );
    sim.running.run(Run::skip_and_resolve()).assert_idle();

    assert_eq!(sim.recorder.connect_failures().len(), 1);
    assert_eq!(sim.recorder.connect_failures()[0].0, peer);
    assert!(sim.recorder.established().is_empty());
    assert!(sim.recorder.closed().is_empty());
}

#[test]
fn rejected_outbound_duplicate_death_does_not_fail_the_live_connection() {
    let mut sim = harness();
    virtual_children(&mut sim);
    let peer = Peer::for_test(4110);
    let (live, extra_id) = ids();
    sim.running.enqueue_msg(
        &sim.manager,
        [
            ManagerMessage::AddPeer(peer),
            connection_ok(peer, 1, live),
            handshake(peer, sim.connection.clone(), live, Role::Initiator, true),
            handshake(peer, sim.extra.clone(), extra_id, Role::Initiator, true),
            ManagerMessage::ConnectionDied(peer, extra_id, Role::Initiator),
            ManagerMessage::ConnectionDied(peer, live, Role::Initiator),
        ],
    );
    sim.running.run(Run::skip_and_resolve()).assert_idle();

    assert!(sim.recorder.connect_failures().is_empty(), "duplicate death is not a failed dial");
    let at = observed_at(sim.running.now());
    assert_eq!(sim.recorder.established().len(), 1);
    assert_eq!(sim.recorder.established()[0].0.conn_id, live);
    assert_eq!(sim.recorder.closed(), vec![(peer, live, CloseReason::BearerEnded, at)]);
}

#[test]
fn inbound_handshake_is_refused_past_the_hard_limit() {
    let mut sim = harness_with(ManagerConfig::default().with_max_inbound(1));
    let first = Peer::for_test(4201);
    let grace = Peer::for_test(4202);
    let refused = Peer::for_test(4203);
    let ids = id_n(3);
    let (live, grace_id, refused_id) = (ids[0], ids[1], ids[2]);
    virtual_children(&mut sim);
    sim.running.enqueue_msg(
        &sim.manager,
        [
            ManagerMessage::Accepted(first, live),
            ManagerMessage::Accepted(grace, grace_id),
            ManagerMessage::Accepted(refused, refused_id),
            handshake(first, sim.connection.clone(), live, Role::Responder, false),
            handshake(grace, StageRef::blackhole(), grace_id, Role::Responder, false),
            handshake(refused, sim.extra.clone(), refused_id, Role::Responder, false),
        ],
    );
    // The grace bearer's disconnect is scheduled; skipping wakeups delivers it to the blackhole.
    sim.running.run(Run::skip_and_resolve()).assert_idle();

    assert_eq!(sim.recorder.established().len(), 1);
    assert_eq!(sim.recorder.established()[0].0.conn_id, live);
    assert_eq!(sim.recorder.established()[0].0.peer, first);
    assert!(sim.recorder.established().iter().all(|(record, _)| record.peer != grace));
    assert!(sim.recorder.closed().is_empty());
    assert!(sim.connection_rx.drain().next().is_none());
    let extra: Vec<_> = sim.extra_rx.drain().collect();
    assert_eq!(extra, vec![ConnectionMessage::Disconnect]);
}

#[test]
fn inbound_death_before_handshake_writes_nothing() {
    let mut sim = harness();
    let peer = Peer::for_test(4109);
    let (conn_id, _) = ids();
    sim.running.enqueue_msg(&sim.manager, [ManagerMessage::ConnectionDied(peer, conn_id, Role::Responder)]);
    sim.running.run(Run::skip_and_resolve()).assert_idle();

    assert!(sim.recorder.established().is_empty());
    assert!(sim.recorder.closed().is_empty());
    assert!(sim.recorder.connect_failures().is_empty());
}

/// `RemovePeer` while a dial is in flight, then `AddPeer`. The old connect result is not the new dial.
#[test]
fn a_cleared_dial_is_not_failed_by_its_old_result() {
    let mut sim = harness();
    virtual_children(&mut sim);
    let peer = Peer::for_test(4301);
    let (old_id, new_id) = ids();

    sim.running.enqueue_msg(
        &sim.manager,
        [
            ManagerMessage::AddPeer(peer),
            ManagerMessage::RemovePeer(peer),
            ManagerMessage::AddPeer(peer),
            connection_err(peer, 1),
            connection_ok(peer, 1, old_id),
            handshake(peer, sim.extra.clone(), old_id, Role::Initiator, true),
        ],
    );
    sim.running.run(Run::skip_and_resolve()).assert_idle();

    assert!(sim.recorder.connect_failures().is_empty(), "the old dial writes no failure");
    assert!(sim.recorder.established().is_empty(), "the old socket is not the live outbound");
    assert!(sim.recorder.closed().is_empty());
    let state = sim.running.get_state(&sim.manager).expect("manager idle");
    match &state.peers[&peer].outbound {
        OutboundState::Scheduled { attempt: 2, conn_id: None } => {}
        other => panic!("new attempt was touched: {other:?}"),
    }

    sim.running.enqueue_msg(
        &sim.manager,
        [connection_ok(peer, 2, new_id), handshake(peer, sim.connection.clone(), new_id, Role::Initiator, true)],
    );
    sim.running.run(Run::skip_and_resolve()).assert_idle();

    assert_eq!(sim.recorder.established().len(), 1);
    assert_eq!(sim.recorder.established()[0].0.conn_id, new_id);
    assert!(sim.recorder.connect_failures().is_empty());
}

/// The socket id was known, then `RemovePeer` dropped it. A later death or handshake is not the next dial.
#[test]
fn a_dropped_socket_death_does_not_fail_the_next_dial() {
    let mut sim = harness();
    virtual_children(&mut sim);
    let peer = Peer::for_test(4302);
    let (old_id, new_id) = ids();

    sim.running.enqueue_msg(
        &sim.manager,
        [
            ManagerMessage::AddPeer(peer),
            connection_ok(peer, 1, old_id),
            ManagerMessage::RemovePeer(peer),
            ManagerMessage::AddPeer(peer),
            ManagerMessage::ConnectionDied(peer, old_id, Role::Initiator),
            handshake(peer, sim.extra.clone(), old_id, Role::Initiator, true),
        ],
    );
    sim.running.run(Run::skip_and_resolve()).assert_idle();

    assert!(sim.recorder.connect_failures().is_empty());
    assert!(sim.recorder.established().is_empty());
    assert!(sim.recorder.closed().is_empty());
    let state = sim.running.get_state(&sim.manager).expect("manager idle");
    match &state.peers[&peer].outbound {
        OutboundState::Scheduled { attempt: 2, conn_id: None } => {}
        other => panic!("new attempt was touched: {other:?}"),
    }

    sim.running.enqueue_msg(
        &sim.manager,
        [connection_ok(peer, 2, new_id), handshake(peer, sim.connection.clone(), new_id, Role::Initiator, true)],
    );
    sim.running.run(Run::skip_and_resolve()).assert_idle();

    assert_eq!(sim.recorder.connect_failures().len(), 0);
    assert_eq!(sim.recorder.established().len(), 1);
    assert_eq!(sim.recorder.established()[0].0.conn_id, new_id);
}

/// A replaced inbound dies after the outbound slot has moved.
/// That death is not a failed dial, and it does not drop the inbound that replaced it.
#[test]
fn replaced_inbound_death_after_the_outbound_slot_moves_is_not_a_failed_dial() {
    let mut sim = harness();
    virtual_children(&mut sim);
    let peer = Peer::for_test(4303);
    let ids = id_n(4);
    let (inbound, live, replaced_by, renewed) = (ids[0], ids[1], ids[2], ids[3]);
    let live_stage = StageRef::named_for_tests("live");
    let renewed_stage = StageRef::named_for_tests("renewed");

    sim.running.enqueue_msg(
        &sim.manager,
        [
            ManagerMessage::Accepted(peer, inbound),
            handshake(peer, sim.connection.clone(), inbound, Role::Responder, false),
            ManagerMessage::AddPeer(peer),
            connection_ok(peer, 1, live),
            handshake(peer, live_stage, live, Role::Initiator, true),
            ManagerMessage::Accepted(peer, replaced_by),
            handshake(peer, sim.extra.clone(), replaced_by, Role::Responder, false),
            ManagerMessage::ConnectionDied(peer, live, Role::Initiator),
            ManagerMessage::AddPeer(peer),
            ManagerMessage::ConnectionDied(peer, inbound, Role::Responder),
        ],
    );
    sim.running.run(Run::skip_and_resolve()).assert_idle();

    assert!(sim.extra_rx.drain().next().is_none(), "the replacement stays connected");
    let replaced: Vec<_> = sim.connection_rx.drain().collect();
    assert_eq!(replaced, vec![ConnectionMessage::Disconnect]);
    assert!(sim.recorder.connect_failures().is_empty());
    let at = observed_at(sim.running.now());
    assert_eq!(
        sim.recorder.closed(),
        vec![(peer, inbound, CloseReason::LocalDisconnect, at), (peer, live, CloseReason::BearerEnded, at),]
    );
    assert!(sim.recorder.established().iter().any(|(record, _)| record.conn_id == replaced_by));

    let state = sim.running.get_state(&sim.manager).expect("manager idle");
    assert_eq!(state.peers[&peer].inbound, Some(replaced_by));
    match &state.peers[&peer].outbound {
        OutboundState::Scheduled { attempt: 2, conn_id: None } => {}
        other => panic!("new attempt was touched: {other:?}"),
    }

    sim.running.enqueue_msg(
        &sim.manager,
        [connection_ok(peer, 2, renewed), handshake(peer, renewed_stage, renewed, Role::Initiator, true)],
    );
    sim.running.run(Run::skip_and_resolve()).assert_idle();

    assert!(sim.recorder.connect_failures().is_empty());
    assert!(sim.recorder.established().iter().any(|(record, _)| record.conn_id == renewed));
    assert!(sim.recorder.established().iter().any(|(record, _)| record.conn_id == replaced_by));
    assert_eq!(sim.recorder.closed().len(), 2);
    let state = sim.running.get_state(&sim.manager).expect("manager idle");
    assert_eq!(state.peers[&peer].inbound, Some(replaced_by));
}

#[test]
fn inbound_hard_limit_is_thirty_percent_and_at_least_one_extra() {
    assert_eq!(super::inbound_hard_limit(0), 1);
    assert_eq!(super::inbound_hard_limit(1), 2);
    assert_eq!(super::inbound_hard_limit(3), 4);
    assert_eq!(super::inbound_hard_limit(4), 5);
    assert_eq!(super::inbound_hard_limit(10), 13);
    assert_eq!(super::inbound_hard_limit(50), 65);
    assert_eq!(super::inbound_hard_limit(100), 130);
}

/// Soft limit 10 keeps ten peers. The next three answer without being recorded, then close.
/// The fourteenth handshake is refused. A slot freed by a kept peer can be recorded again.
#[test]
fn inbound_between_soft_and_hard_answers_briefly_then_closes() {
    let mut sim = harness_with(ManagerConfig::default().with_max_inbound(10));
    virtual_children(&mut sim);
    let ids = id_n(15);
    let durable: Vec<_> = (0..10).map(|i| (Peer::for_test(6000 + i as u16), ids[i])).collect();
    let transient: Vec<_> = (0..3).map(|i| (Peer::for_test(6100 + i as u16), ids[10 + i])).collect();
    let refused = (Peer::for_test(6200), ids[13]);
    let renewed = (Peer::for_test(6201), ids[14]);

    let mut setup = Vec::new();
    for (peer, conn_id) in &durable {
        setup.push(ManagerMessage::Accepted(*peer, *conn_id));
    }
    for (peer, conn_id) in &durable {
        setup.push(handshake(*peer, StageRef::blackhole(), *conn_id, Role::Responder, false));
    }
    sim.running.enqueue_msg(&sim.manager, setup);
    sim.running.run(Run::skip_and_resolve()).assert_idle();
    assert_eq!(sim.recorder.established().len(), 10);

    let mut overflow = vec![
        ManagerMessage::Accepted(transient[0].0, transient[0].1),
        ManagerMessage::Accepted(transient[1].0, transient[1].1),
        ManagerMessage::Accepted(transient[2].0, transient[2].1),
        ManagerMessage::Accepted(refused.0, refused.1),
    ];
    for (peer, conn_id) in &transient {
        overflow.push(handshake(*peer, sim.connection.clone(), *conn_id, Role::Responder, false));
    }
    overflow.push(handshake(refused.0, sim.extra.clone(), refused.1, Role::Responder, false));
    sim.running.enqueue_msg(&sim.manager, overflow);

    let started = sim.running.now();
    sim.running
        .run(Run { time: TimeAdvance::StopAtWakeup, externals: Externals::Resolve })
        .assert_sleeping_until(started + super::TRANSIENT_INBOUND_TTL);
    assert_eq!(sim.recorder.established().len(), 10, "grace bearers are not downstream peers");
    assert!(sim.connection_rx.drain().next().is_none(), "the grace window has not elapsed");
    let refused_msgs: Vec<_> = sim.extra_rx.drain().collect();
    assert_eq!(refused_msgs, vec![ConnectionMessage::Disconnect]);

    assert!(sim.running.skip_to_next_wakeup(None));
    sim.running.run(Run { time: TimeAdvance::StopAtWakeup, externals: Externals::Resolve }).assert_idle();
    let closed: Vec<_> = sim.connection_rx.drain().collect();
    assert_eq!(closed.len(), 3, "each grace bearer is disconnected");
    assert!(closed.iter().all(|msg| matches!(msg, ConnectionMessage::Disconnect)));
    assert!(sim.recorder.closed().is_empty());

    sim.running.enqueue_msg(
        &sim.manager,
        transient.iter().map(|(peer, conn_id)| ManagerMessage::ConnectionDied(*peer, *conn_id, Role::Responder)),
    );
    sim.running.run(Run::skip_and_resolve()).assert_idle();
    assert_eq!(sim.recorder.established().len(), 10);
    assert!(sim.recorder.closed().is_empty(), "a grace bearer was never established");

    // The ten kept peers still fill the soft limit. Free one so the next inbound is kept.
    sim.running
        .enqueue_msg(&sim.manager, [ManagerMessage::ConnectionDied(durable[0].0, durable[0].1, Role::Responder)]);
    sim.running.run(Run::skip_and_resolve()).assert_idle();
    assert_eq!(sim.recorder.closed().len(), 1);

    sim.running.enqueue_msg(
        &sim.manager,
        [
            ManagerMessage::Accepted(renewed.0, renewed.1),
            handshake(renewed.0, StageRef::blackhole(), renewed.1, Role::Responder, false),
        ],
    );
    sim.running.run(Run::skip_and_resolve()).assert_idle();
    assert_eq!(sim.recorder.established().len(), 11);
    assert!(sim.recorder.established().iter().any(|(record, _)| record.conn_id == renewed.1));
}
