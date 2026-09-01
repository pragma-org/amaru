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

//! Discrete-event engine tests (heap, wire, delays). No chain data.
//!
//! Generated vs recorded chain tests live in `generated` and `real_data`.
//! See EDR-011 "World tests: generated vs recorded chains".

use std::{net::SocketAddr, num::NonZeroUsize, sync::Arc, time::Duration};

use amaru_kernel::{NonEmptyBytes, PREPROD_ERA_HISTORY, Peer};
use amaru_ouroboros::{ConnectionId, ConnectionsResource};
use amaru_protocols::network_effects::{
    AcceptEffect, AcceptError, ConnectEffect, ConnectError, ListenEffect, ListenError, Network, NetworkOps,
    ReceiveError, RecvEffect, SendEffect, SendError,
};
use amaru_pure_stage::{
    Effect, Instant, Name, StageGraph, StageResponse, assert_trace_match_filter, register_data_deserializer,
    register_effect_deserializer,
    simulation::{Fifo, SimulationBuilder},
    tm_clock, tm_effect, tm_input, tm_resume_external, tm_resume_unit, tm_state,
    trace_buffer::{TraceBuffer, TraceEntry},
};
use parking_lot::Mutex;
use tokio_util::bytes::Bytes;

use super::{
    GraphWakeReason, HONEST_PAYLOAD_DELAY_MAX_NANOS, HONEST_PAYLOAD_DELAY_SLOTS, HeapLogEntry, HeapLogKind,
    LONG_TAIL_PAYLOAD_EVERY, LONG_TAIL_PAYLOAD_MIN_NANOS, NetworkEvent, WIRE_DELAY_MAX_NANOS, WIRE_DELAY_MIN_NANOS,
    WorldConnectionProvider, WorldLoop, long_tail_payload_delay_nanos, wire_delay_nanos,
};

const SEED: u64 = 0xA11CE;

type Observed<T> = Arc<Mutex<Option<T>>>;

fn observed<T>() -> Observed<T> {
    Arc::new(Mutex::new(None))
}

fn set_observed<T>(slot: &Observed<T>, value: T) {
    *slot.lock() = Some(value);
}

fn provider() -> Arc<WorldConnectionProvider> {
    Arc::new(WorldConnectionProvider::new(SEED))
}

fn by_time_seq(mut log: Vec<HeapLogEntry>) -> Vec<HeapLogEntry> {
    log.sort_by_key(|e| (e.time_nanos, e.sequence));
    log
}

fn assert_heap_log(actual: Vec<HeapLogEntry>, expected: Vec<HeapLogEntry>) {
    assert_eq!(by_time_seq(actual), by_time_seq(expected));
}

fn graph_wake(sequence: u64, time_nanos: u64, graph: usize, reason: GraphWakeReason) -> HeapLogEntry {
    HeapLogEntry { sequence, time_nanos, kind: HeapLogKind::GraphWake { graph, reason } }
}

fn pair_ids() -> (ConnectionId, ConnectionId) {
    let mut ids = ConnectionId::initial();
    (ids.get_and_increment(), ids.get_and_increment())
}

fn initiator_addr(initiator: ConnectionId) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 5000 + initiator.as_u64() as u16))
}

fn peer_addr(addr: SocketAddr) -> Peer {
    Peer::try_from(addr).expect("world tests use IPv4 loopback")
}

/// Prove one Deliver round-trip under the world loop.
/// Node A listens+accepts+recv, Node B connects+sends. Driven only by WorldLoop.
fn trace_guards() -> amaru_pure_stage::DeserializerGuards {
    let mut guards = amaru_protocols::network_effects::register_deserializers();
    guards.push(register_data_deserializer::<()>().boxed());
    guards.push(register_data_deserializer::<Result<SocketAddr, ListenError>>().boxed());
    guards.push(register_data_deserializer::<Result<ConnectionId, ConnectError>>().boxed());
    guards.push(register_data_deserializer::<Result<(Peer, ConnectionId), AcceptError>>().boxed());
    guards.push(register_data_deserializer::<Result<(), SendError>>().boxed());
    guards.push(register_data_deserializer::<Result<NonEmptyBytes, ReceiveError>>().boxed());
    guards.push(register_effect_deserializer::<ListenEffect>().boxed());
    guards.push(register_effect_deserializer::<AcceptEffect>().boxed());
    guards.push(register_effect_deserializer::<ConnectEffect>().boxed());
    guards.push(register_effect_deserializer::<SendEffect>().boxed());
    guards.push(register_effect_deserializer::<RecvEffect>().boxed());
    guards
}

#[tokio::test]
async fn test_one_deliver_roundtrip_with_world_loop() {
    let _guards = trace_guards();
    let handle = tokio::runtime::Handle::current();
    let provider = provider();
    let trace = TraceBuffer::new_shared(100, 1_000_000);
    let listener_addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();
    let received = observed::<Vec<u8>>();
    let received_a = received.clone();

    let mut stage_graph_a = SimulationBuilder::default().with_trace_buffer(trace.clone()).with_eval_strategy(Fifo);
    stage_graph_a.resources().put::<ConnectionsResource>(provider.clone());

    let stage_a = stage_graph_a.stage("node_a", move |_state: (), _unit: (), eff| {
        let received_a = received_a.clone();
        async move {
            let net = Network::new(&eff);
            net.listen(listener_addr).await.unwrap();
            let (_peer, conn) = net.accept(listener_addr).await.unwrap();
            let msg_len = NonZeroUsize::new("hello from B".len()).unwrap();
            let bytes = net.recv(conn, msg_len).await.unwrap();
            set_observed(&received_a, bytes.as_ref().to_vec());
        }
    });
    let stage_a = stage_graph_a.wire_up(stage_a, ());
    let mut sim_a = stage_graph_a.run(&handle);
    sim_a.enqueue_msg(&stage_a, [()]);

    let mut stage_graph_b = SimulationBuilder::default().with_trace_buffer(trace.clone()).with_eval_strategy(Fifo);
    stage_graph_b.resources().put::<ConnectionsResource>(provider.clone());

    let stage_b = stage_graph_b.stage("node_b", move |_state: (), _unit: (), eff| async move {
        let net = Network::new(&eff);
        let conn = net.connect(peer_addr(listener_addr), Duration::from_secs(1)).await.unwrap();
        let msg = NonEmptyBytes::try_from(Bytes::from("hello from B")).unwrap();
        net.send(conn, msg).await.unwrap();
    });
    let stage_b = stage_graph_b.wire_up(stage_b, ());
    let mut sim_b = stage_graph_b.run(&handle);
    sim_b.enqueue_msg(&stage_b, [()]);

    let mut world = WorldLoop::new(provider, vec![sim_a, sim_b]);
    world.run_to_completion();

    assert_eq!(received.lock().as_deref(), Some(b"hello from B".as_ref()));

    let (initiator, responder) = pair_ids();
    let initiator_sock = initiator_addr(initiator);
    let d_connected = wire_delay_nanos(SEED, 0);
    let d_accepted = wire_delay_nanos(SEED, 1);
    let d_deliver = wire_delay_nanos(SEED, 2);
    let t_connected = d_connected;
    let t_accepted = t_connected + d_accepted;
    let t_deliver = t_connected + d_deliver;
    let msg = NonEmptyBytes::try_from(Bytes::from("hello from B")).unwrap();
    let mut expected_log = vec![
        graph_wake(0, 0, 0, GraphWakeReason::Runnable),
        graph_wake(1, 0, 1, GraphWakeReason::Runnable),
        HeapLogEntry {
            sequence: 2,
            time_nanos: t_connected,
            kind: HeapLogKind::ConnectAttempt { target: listener_addr },
        },
        graph_wake(5, t_connected, 1, GraphWakeReason::Runnable),
        HeapLogEntry { sequence: 6, time_nanos: t_connected, kind: HeapLogKind::SendAck { conn: initiator } },
        graph_wake(8, t_connected, 1, GraphWakeReason::Runnable),
        HeapLogEntry {
            sequence: 4,
            time_nanos: t_accepted,
            kind: HeapLogKind::Accepted {
                listener: listener_addr,
                responder_conn: responder,
                initiator_addr: initiator_sock,
            },
        },
        graph_wake(9, t_accepted, 0, GraphWakeReason::Runnable),
        HeapLogEntry {
            sequence: 7,
            time_nanos: t_deliver,
            kind: HeapLogKind::Deliver { conn: responder, data_len: 12 },
        },
    ];
    if t_deliver > t_accepted {
        expected_log.push(graph_wake(10, t_deliver, 0, GraphWakeReason::Runnable));
    }
    assert_heap_log(world.take_heap_log(), expected_log);

    let mut expected = Vec::new();
    expected.extend([
        tm_state("node_a-1", &()),
        tm_state("node_b-1", &()),
        tm_input("node_a-1", &()),
        tm_input("node_b-1", &()),
        tm_resume_unit("node_a-1"),
        tm_effect("node_a-1", ListenEffect { addr: listener_addr }),
        tm_resume_external("node_a-1", Ok::<SocketAddr, ListenError>(listener_addr)),
        tm_effect("node_a-1", AcceptEffect { listener_addr }),
        tm_resume_unit("node_b-1"),
        tm_effect("node_b-1", ConnectEffect { peer: peer_addr(listener_addr), timeout: Duration::from_secs(1) }),
        tm_clock(Duration::from_nanos(t_connected)),
        tm_resume_external("node_b-1", Ok::<ConnectionId, ConnectError>(initiator)),
        tm_effect("node_b-1", SendEffect { conn: initiator, data: msg.clone() }),
        tm_resume_external("node_b-1", Ok::<(), SendError>(())),
        tm_state("node_b-1", &()),
    ]);
    if t_accepted <= t_deliver {
        expected.extend([
            tm_clock(Duration::from_nanos(t_accepted)),
            tm_resume_external(
                "node_a-1",
                Ok::<(Peer, ConnectionId), AcceptError>((peer_addr(initiator_sock), responder)),
            ),
            tm_effect("node_a-1", RecvEffect { conn: responder, bytes: NonZeroUsize::new(12).unwrap() }),
        ]);
        if t_deliver > t_accepted {
            expected.extend([tm_clock(Duration::from_nanos(t_deliver))]);
        }
        expected.extend([
            tm_resume_external("node_a-1", Ok::<NonEmptyBytes, ReceiveError>(msg)),
            tm_state("node_a-1", &()),
        ]);
    } else {
        expected.extend([
            tm_clock(Duration::from_nanos(t_accepted)),
            tm_resume_external(
                "node_a-1",
                Ok::<(Peer, ConnectionId), AcceptError>((peer_addr(initiator_sock), responder)),
            ),
            tm_effect("node_a-1", RecvEffect { conn: responder, bytes: NonZeroUsize::new(12).unwrap() }),
            tm_resume_external("node_a-1", Ok::<NonEmptyBytes, ReceiveError>(msg)),
            tm_state("node_a-1", &()),
        ]);
    }
    assert_trace_match_filter(world.graph(0), &expected, &[]);
}

/// Horizon cuts keepalive.
///
/// Schedule two explicit heap events:
/// - keepalive at t_in=100 (≤ H=1000)
/// - another at t_out=1500 (> H=1000)
///
/// After run_until_horizon(H=1000):
/// - t_in=100 event is in heap_log
/// - peek_next_event_time() returns Some(1500) (still on heap)
#[tokio::test]
async fn test_horizon_cuts_keepalive() {
    let provider = provider();

    let conn_in = ConnectionId::initial();
    provider.schedule_event_at(100, NetworkEvent::Close { conn: conn_in });

    let conn_out = ConnectionId::initial();
    provider.schedule_event_at(1500, NetworkEvent::Close { conn: conn_out });

    let mut world = WorldLoop::new(provider, vec![]);
    world.run_until_horizon(1000);

    assert_heap_log(
        world.take_heap_log(),
        vec![HeapLogEntry { sequence: 0, time_nanos: 100, kind: HeapLogKind::Close { conn: conn_in } }],
    );
    assert_eq!(world.peek_next_event_time(), Some(1500), "Event at t=1500 should still be on heap (not popped)");
}

/// Connected completes the pending connect for that listener, not a process-wide FIFO.
#[tokio::test]
async fn test_pending_connect_matches_listener() {
    let handle = tokio::runtime::Handle::current();
    let provider = provider();
    let addr1: SocketAddr = "127.0.0.1:9101".parse().unwrap();
    let addr2: SocketAddr = "127.0.0.1:9102".parse().unwrap();
    let got1 = observed::<Vec<u8>>();
    let got2 = observed::<Vec<u8>>();
    let g1 = got1.clone();
    let g2 = got2.clone();

    let listen = |name: &'static str, addr: SocketAddr, slot: Observed<Vec<u8>>| {
        let provider = provider.clone();
        let mut graph = SimulationBuilder::default().with_eval_strategy(Fifo);
        graph.resources().put::<ConnectionsResource>(provider);
        let stage = graph.stage(name, move |_state: (), _unit: (), eff| {
            let slot = slot.clone();
            async move {
                let net = Network::new(&eff);
                net.listen(addr).await.unwrap();
                let (_peer, conn) = net.accept(addr).await.unwrap();
                let bytes = net.recv(conn, NonZeroUsize::new(3).unwrap()).await.unwrap();
                set_observed(&slot, bytes.as_ref().to_vec());
            }
        });
        let stage = graph.wire_up(stage, ());
        let mut sim = graph.run(&handle);
        sim.enqueue_msg(&stage, [()]);
        sim
    };
    let connect = |name: &'static str, addr: SocketAddr, payload: &'static [u8]| {
        let provider = provider.clone();
        let mut graph = SimulationBuilder::default().with_eval_strategy(Fifo);
        graph.resources().put::<ConnectionsResource>(provider);
        let stage = graph.stage(name, move |_state: (), _unit: (), eff| async move {
            let net = Network::new(&eff);
            let conn = net.connect(peer_addr(addr), Duration::from_secs(1)).await.unwrap();
            net.send(conn, NonEmptyBytes::try_from(Bytes::from(payload)).unwrap()).await.unwrap();
        });
        let stage = graph.wire_up(stage, ());
        let mut sim = graph.run(&handle);
        sim.enqueue_msg(&stage, [()]);
        sim
    };

    let mut world = WorldLoop::new(
        provider.clone(),
        vec![
            connect("c1", addr1, b"one"),
            connect("c2", addr2, b"two"),
            listen("l1", addr1, g1),
            listen("l2", addr2, g2),
        ],
    );
    world.run_to_completion();
    assert_eq!(got1.lock().as_deref(), Some(b"one".as_ref()));
    assert_eq!(got2.lock().as_deref(), Some(b"two".as_ref()));
}

/// Connect is a wire hop: listen after send but before SYN arrival still succeeds.
#[tokio::test]
async fn test_listen_before_connect_attempt_arrives() {
    let _guards = trace_guards();
    let handle = tokio::runtime::Handle::current();
    let provider = provider();
    let trace = TraceBuffer::new_shared(100, 1_000_000);
    let listener_addr: SocketAddr = "127.0.0.1:9110".parse().unwrap();
    let received = observed::<Vec<u8>>();
    let received_a = received.clone();

    let mut graph_b = SimulationBuilder::default().with_trace_buffer(trace.clone()).with_eval_strategy(Fifo);
    graph_b.resources().put::<ConnectionsResource>(provider.clone());
    let stage_b = graph_b.stage("node_b", move |_state: (), _unit: (), eff| async move {
        let net = Network::new(&eff);
        let conn = net.connect(peer_addr(listener_addr), Duration::from_secs(1)).await.unwrap();
        net.send(conn, NonEmptyBytes::try_from(Bytes::from("ok")).unwrap()).await.unwrap();
    });
    let stage_b = graph_b.wire_up(stage_b, ());
    let mut sim_b = graph_b.run(&handle);
    sim_b.enqueue_msg(&stage_b, [()]);

    let mut graph_a = SimulationBuilder::default().with_trace_buffer(trace.clone()).with_eval_strategy(Fifo);
    graph_a.resources().put::<ConnectionsResource>(provider.clone());
    let stage_a = graph_a.stage("node_a", move |_state: (), _unit: (), eff| {
        let received_a = received_a.clone();
        async move {
            let net = Network::new(&eff);
            net.listen(listener_addr).await.unwrap();
            let (_peer, conn) = net.accept(listener_addr).await.unwrap();
            let bytes = net.recv(conn, NonZeroUsize::new(2).unwrap()).await.unwrap();
            set_observed(&received_a, bytes.as_ref().to_vec());
        }
    });
    let stage_a = graph_a.wire_up(stage_a, ());
    let mut sim_a = graph_a.run(&handle);
    sim_a.enqueue_msg(&stage_a, [()]);

    let mut world = WorldLoop::new(provider, vec![sim_b, sim_a]);
    world.run_to_completion();
    assert_eq!(received.lock().as_deref(), Some(b"ok".as_ref()));

    let (initiator, responder) = pair_ids();
    let initiator_sock = initiator_addr(initiator);
    let t_attempt = wire_delay_nanos(SEED, 0);
    let t_accepted = t_attempt + wire_delay_nanos(SEED, 1);
    let t_deliver = t_attempt + wire_delay_nanos(SEED, 2);
    assert!((WIRE_DELAY_MIN_NANOS..=WIRE_DELAY_MAX_NANOS).contains(&t_attempt));
    let log = by_time_seq(world.take_heap_log());
    let attempt = log
        .iter()
        .find(|e| e.kind == HeapLogKind::ConnectAttempt { target: listener_addr })
        .expect("ConnectAttempt on the unified heap");
    assert_eq!(attempt.time_nanos, t_attempt);
    assert!(log.iter().any(|e| e.kind == HeapLogKind::SendAck { conn: initiator }));
    assert!(log.iter().any(|e| matches!(e.kind, HeapLogKind::GraphWake { .. })));

    let msg = NonEmptyBytes::try_from(Bytes::from("ok")).unwrap();
    let mut expected = Vec::new();
    expected.extend([
        tm_state("node_b-1", &()),
        tm_state("node_a-1", &()),
        tm_input("node_b-1", &()),
        tm_input("node_a-1", &()),
        tm_resume_unit("node_b-1"),
        tm_effect("node_b-1", ConnectEffect { peer: peer_addr(listener_addr), timeout: Duration::from_secs(1) }),
        tm_resume_unit("node_a-1"),
        tm_effect("node_a-1", ListenEffect { addr: listener_addr }),
        tm_resume_external("node_a-1", Ok::<SocketAddr, ListenError>(listener_addr)),
        tm_effect("node_a-1", AcceptEffect { listener_addr }),
        tm_clock(Duration::from_nanos(t_attempt)),
        tm_resume_external("node_b-1", Ok::<ConnectionId, ConnectError>(initiator)),
        tm_effect("node_b-1", SendEffect { conn: initiator, data: msg.clone() }),
        tm_resume_external("node_b-1", Ok::<(), SendError>(())),
        tm_state("node_b-1", &()),
    ]);
    if t_accepted <= t_deliver {
        expected.extend([
            tm_clock(Duration::from_nanos(t_accepted)),
            tm_resume_external(
                "node_a-1",
                Ok::<(Peer, ConnectionId), AcceptError>((peer_addr(initiator_sock), responder)),
            ),
            tm_effect("node_a-1", RecvEffect { conn: responder, bytes: NonZeroUsize::new(2).unwrap() }),
        ]);
        if t_deliver > t_accepted {
            expected.extend([tm_clock(Duration::from_nanos(t_deliver))]);
        }
        expected.extend([
            tm_resume_external("node_a-1", Ok::<NonEmptyBytes, ReceiveError>(msg)),
            tm_state("node_a-1", &()),
        ]);
    } else {
        expected.extend([
            tm_clock(Duration::from_nanos(t_accepted)),
            tm_resume_external(
                "node_a-1",
                Ok::<(Peer, ConnectionId), AcceptError>((peer_addr(initiator_sock), responder)),
            ),
            tm_effect("node_a-1", RecvEffect { conn: responder, bytes: NonZeroUsize::new(2).unwrap() }),
            tm_resume_external("node_a-1", Ok::<NonEmptyBytes, ReceiveError>(msg)),
            tm_state("node_a-1", &()),
        ]);
    }
    assert_trace_match_filter(world.graph(0), &expected, &[]);
}

/// Connect completes and the initiator sends before accept; bytes must still arrive.
#[tokio::test]
async fn test_send_before_accept_delivers() {
    let handle = tokio::runtime::Handle::current();
    let provider = provider();
    let listener_addr: SocketAddr = "127.0.0.1:9200".parse().unwrap();
    let received = observed::<Vec<u8>>();
    let received_a = received.clone();

    let mut graph_a = SimulationBuilder::default().with_eval_strategy(Fifo);
    graph_a.resources().put::<ConnectionsResource>(provider.clone());
    let stage_a = graph_a.stage("node_a", move |_state: (), _unit: (), eff| {
        let received_a = received_a.clone();
        async move {
            let net = Network::new(&eff);
            net.listen(listener_addr).await.unwrap();
            eff.wait(Duration::from_nanos(10_000_000)).await;
            let (_peer, conn) = net.accept(listener_addr).await.unwrap();
            let bytes = net.recv(conn, NonZeroUsize::new(4).unwrap()).await.unwrap();
            set_observed(&received_a, bytes.as_ref().to_vec());
        }
    });
    let stage_a = graph_a.wire_up(stage_a, ());
    let mut sim_a = graph_a.run(&handle);
    sim_a.enqueue_msg(&stage_a, [()]);

    let mut graph_b = SimulationBuilder::default().with_eval_strategy(Fifo);
    graph_b.resources().put::<ConnectionsResource>(provider.clone());
    let stage_b = graph_b.stage("node_b", move |_state: (), _unit: (), eff| async move {
        let net = Network::new(&eff);
        let conn = net.connect(peer_addr(listener_addr), Duration::from_secs(1)).await.unwrap();
        net.send(conn, NonEmptyBytes::try_from(Bytes::from("ping")).unwrap()).await.unwrap();
    });
    let stage_b = graph_b.wire_up(stage_b, ());
    let mut sim_b = graph_b.run(&handle);
    sim_b.enqueue_msg(&stage_b, [()]);

    let mut world = WorldLoop::new(provider, vec![sim_a, sim_b]);
    world.run_to_completion();
    assert_eq!(received.lock().as_deref(), Some(b"ping".as_ref()));

    let (initiator, responder) = pair_ids();
    let initiator_addr = initiator_addr(initiator);
    let d_connected = wire_delay_nanos(SEED, 0);
    let d_deliver = wire_delay_nanos(SEED, 1);
    let d_accepted = wire_delay_nanos(SEED, 2);
    assert_heap_log(
        world.take_heap_log(),
        vec![
            graph_wake(0, 0, 0, GraphWakeReason::Runnable),
            graph_wake(1, 0, 1, GraphWakeReason::Runnable),
            graph_wake(2, 10_000_000, 0, GraphWakeReason::Sleeping),
            HeapLogEntry {
                sequence: 3,
                time_nanos: d_connected,
                kind: HeapLogKind::ConnectAttempt { target: listener_addr },
            },
            graph_wake(5, d_connected, 1, GraphWakeReason::Runnable),
            HeapLogEntry { sequence: 6, time_nanos: d_connected, kind: HeapLogKind::SendAck { conn: initiator } },
            graph_wake(8, d_connected, 1, GraphWakeReason::Runnable),
            HeapLogEntry {
                sequence: 7,
                time_nanos: d_connected + d_deliver,
                kind: HeapLogKind::Deliver { conn: responder, data_len: 4 },
            },
            HeapLogEntry {
                sequence: 9,
                time_nanos: 10_000_000 + d_accepted,
                kind: HeapLogKind::Accepted { listener: listener_addr, responder_conn: responder, initiator_addr },
            },
            graph_wake(10, 10_000_000 + d_accepted, 0, GraphWakeReason::Runnable),
        ],
    );
}

/// Mux-style recv header then leftover body: second recv must complete from the buffer
/// via WorldLoop (recv is never Ready), not a stale pending_recvs entry.
#[tokio::test]
async fn test_mux_recv_header_then_leftover_body() {
    let handle = tokio::runtime::Handle::current();
    let provider = provider();
    let listener_addr: SocketAddr = "127.0.0.1:9300".parse().unwrap();
    let header = observed::<Vec<u8>>();
    let body = observed::<Vec<u8>>();
    let header_a = header.clone();
    let body_a = body.clone();

    let mut graph_a = SimulationBuilder::default().with_eval_strategy(Fifo);
    graph_a.resources().put::<ConnectionsResource>(provider.clone());
    let stage_a = graph_a.stage("node_a", move |_state: (), _unit: (), eff| {
        let header_a = header_a.clone();
        let body_a = body_a.clone();
        async move {
            let net = Network::new(&eff);
            net.listen(listener_addr).await.unwrap();
            let (_peer, conn) = net.accept(listener_addr).await.unwrap();
            let head = net.recv(conn, NonZeroUsize::new(2).unwrap()).await.unwrap();
            set_observed(&header_a, head.as_ref().to_vec());
            let rest = net.recv(conn, NonZeroUsize::new(4).unwrap()).await.unwrap();
            set_observed(&body_a, rest.as_ref().to_vec());
        }
    });
    let stage_a = graph_a.wire_up(stage_a, ());
    let mut sim_a = graph_a.run(&handle);
    sim_a.enqueue_msg(&stage_a, [()]);

    let mut graph_b = SimulationBuilder::default().with_eval_strategy(Fifo);
    graph_b.resources().put::<ConnectionsResource>(provider.clone());
    let stage_b = graph_b.stage("node_b", move |_state: (), _unit: (), eff| async move {
        let net = Network::new(&eff);
        let conn = net.connect(peer_addr(listener_addr), Duration::from_secs(1)).await.unwrap();
        net.send(conn, NonEmptyBytes::try_from(Bytes::from("ABCDEF")).unwrap()).await.unwrap();
    });
    let stage_b = graph_b.wire_up(stage_b, ());
    let mut sim_b = graph_b.run(&handle);
    sim_b.enqueue_msg(&stage_b, [()]);

    let mut world = WorldLoop::new(provider, vec![sim_a, sim_b]);
    world.run_to_completion();
    assert_eq!(header.lock().as_deref(), Some(b"AB".as_ref()));
    assert_eq!(body.lock().as_deref(), Some(b"CDEF".as_ref()));
}

/// Close must fail a parked recv on the peer, not leave it waiting.
#[tokio::test]
async fn test_close_fails_peer_recv() {
    let handle = tokio::runtime::Handle::current();
    let provider = provider();
    let listener_addr: SocketAddr = "127.0.0.1:9400".parse().unwrap();
    let recv_err = observed::<String>();
    let recv_err_a = recv_err.clone();

    let mut graph_a = SimulationBuilder::default().with_eval_strategy(Fifo);
    graph_a.resources().put::<ConnectionsResource>(provider.clone());
    let stage_a = graph_a.stage("node_a", move |_state: (), _unit: (), eff| {
        let recv_err_a = recv_err_a.clone();
        async move {
            let net = Network::new(&eff);
            net.listen(listener_addr).await.unwrap();
            let (_peer, conn) = net.accept(listener_addr).await.unwrap();
            let err = net.recv(conn, NonZeroUsize::new(1).unwrap()).await.unwrap_err();
            set_observed(&recv_err_a, err.to_string());
        }
    });
    let stage_a = graph_a.wire_up(stage_a, ());
    let mut sim_a = graph_a.run(&handle);
    sim_a.enqueue_msg(&stage_a, [()]);

    let mut graph_b = SimulationBuilder::default().with_eval_strategy(Fifo);
    graph_b.resources().put::<ConnectionsResource>(provider.clone());
    let stage_b = graph_b.stage("node_b", move |_state: (), _unit: (), eff| async move {
        let net = Network::new(&eff);
        let conn = net.connect(peer_addr(listener_addr), Duration::from_secs(1)).await.unwrap();
        net.close(conn).await.unwrap();
    });
    let stage_b = graph_b.wire_up(stage_b, ());
    let mut sim_b = graph_b.run(&handle);
    sim_b.enqueue_msg(&stage_b, [()]);

    let mut world = WorldLoop::new(provider, vec![sim_a, sim_b]);
    world.run_to_completion();
    assert!(recv_err.lock().as_ref().is_some_and(|s| s.contains("connection closed")));
}

/// After the far endpoint is gone, a later send must error and the world must settle.
///
/// Closing only the dropped half used to leave the survivor's muxer to ack the last
/// write, start another, then hit `connection reset` without `Written` — the P-join hang.
#[tokio::test]
async fn test_send_after_peer_close_errors_and_settles() {
    let handle = tokio::runtime::Handle::current();
    let provider = provider();
    let listener_addr: SocketAddr = "127.0.0.1:9410".parse().unwrap();
    let send_err = observed::<String>();
    let send_err_b = send_err.clone();
    let recv_err = observed::<String>();
    let recv_err_a = recv_err.clone();
    let (_initiator, responder) = pair_ids();
    let t_connected = wire_delay_nanos(SEED, 0);
    provider.schedule_event_at(t_connected.saturating_add(1_000_000), NetworkEvent::Close { conn: responder });

    let mut graph_a = SimulationBuilder::default().with_eval_strategy(Fifo);
    graph_a.resources().put::<ConnectionsResource>(provider.clone());
    let stage_a = graph_a.stage("node_a", move |_state: (), _unit: (), eff| {
        let recv_err_a = recv_err_a.clone();
        async move {
            let net = Network::new(&eff);
            net.listen(listener_addr).await.unwrap();
            let (_peer, conn) = net.accept(listener_addr).await.unwrap();
            let err = net.recv(conn, NonZeroUsize::new(1).unwrap()).await.unwrap_err();
            set_observed(&recv_err_a, err.to_string());
        }
    });
    let stage_a = graph_a.wire_up(stage_a, ());
    let mut sim_a = graph_a.run(&handle);
    sim_a.enqueue_msg(&stage_a, [()]);

    let mut graph_b = SimulationBuilder::default().with_eval_strategy(Fifo);
    graph_b.resources().put::<ConnectionsResource>(provider.clone());
    let stage_b = graph_b.stage("node_b", move |_state: (), _unit: (), eff| {
        let send_err_b = send_err_b.clone();
        async move {
            let net = Network::new(&eff);
            let conn = net.connect(peer_addr(listener_addr), Duration::from_secs(1)).await.unwrap();
            eff.wait(Duration::from_millis(10)).await;
            let err = net.send(conn, NonEmptyBytes::try_from(Bytes::from("x")).unwrap()).await.unwrap_err();
            set_observed(&send_err_b, err.to_string());
        }
    });
    let stage_b = graph_b.wire_up(stage_b, ());
    let mut sim_b = graph_b.run(&handle);
    sim_b.enqueue_msg(&stage_b, [()]);

    let mut world = WorldLoop::new(provider, vec![sim_a, sim_b]);
    world.run_to_completion();
    assert!(
        send_err.lock().as_ref().is_some_and(|s| s.contains("connection reset") || s.contains("connection closed")),
        "survivor send after peer close: {:?}",
        send_err.lock()
    );
    assert!(
        recv_err.lock().as_ref().is_some_and(|s| s.contains("connection reset") || s.contains("connection closed")),
        "dropped-side recv: {:?}",
        recv_err.lock()
    );
}

/// A graph that terminates while a same-conn Deliver is still on the heap must
/// drop that stage's pending recv. The Deliver still pops and logs; WorldLoop
/// must not resume the gone stage. Live `heap_contents` / `heap_log`, not a
/// sorted-only proof. Without the drop, `resume_external_box` panics.
#[tokio::test]
async fn test_terminate_drops_pending_before_later_deliver() {
    const PAYLOAD_HOP_NANOS: u64 = 10_000_000;
    let handle = tokio::runtime::Handle::current();
    let provider = Arc::new(WorldConnectionProvider::with_payload_delay(SEED, PAYLOAD_HOP_NANOS, PAYLOAD_HOP_NANOS));
    let listener_addr: SocketAddr = "127.0.0.1:9800".parse().unwrap();
    let received = observed::<Vec<u8>>();
    let received_a = received.clone();
    let trace = TraceBuffer::new_shared(100, 1_000_000);

    let mut graph_a = SimulationBuilder::default().with_trace_buffer(trace.clone()).with_eval_strategy(Fifo);
    graph_a.resources().put::<ConnectionsResource>(provider.clone());
    let stage_a = graph_a.stage("parent", move |_state: (), _unit: (), eff| {
        let received_a = received_a.clone();
        async move {
            let net = Network::new(&eff);
            net.listen(listener_addr).await.unwrap();
            let (_peer, conn) = net.accept(listener_addr).await.unwrap();
            let child = eff
                .stage("recv", move |(), conn: ConnectionId, eff| {
                    let received_a = received_a.clone();
                    async move {
                        let net = Network::new(&eff);
                        let bytes = net.recv(conn, NonZeroUsize::new(4).unwrap()).await.unwrap();
                        set_observed(&received_a, bytes.as_ref().to_vec());
                    }
                })
                .await;
            let child = eff.wire_up(child, ()).await;
            eff.send(&child, conn).await;
            eff.wait(Duration::from_nanos(1)).await;
            eff.terminate().await
        }
    });
    let stage_a = graph_a.wire_up(stage_a, ());
    let mut sim_a = graph_a.run(&handle);
    sim_a.enqueue_msg(&stage_a, [()]);

    let mut graph_b = SimulationBuilder::default().with_eval_strategy(Fifo);
    graph_b.resources().put::<ConnectionsResource>(provider.clone());
    let stage_b = graph_b.stage("sender", move |_state: (), _unit: (), eff| async move {
        let net = Network::new(&eff);
        let conn = net.connect(peer_addr(listener_addr), Duration::from_secs(1)).await.unwrap();
        net.send(conn, NonEmptyBytes::try_from(Bytes::from("ping")).unwrap()).await.unwrap();
    });
    let stage_b = graph_b.wire_up(stage_b, ());
    let mut sim_b = graph_b.run(&handle);
    sim_b.enqueue_msg(&stage_b, [()]);

    let mut world = WorldLoop::new(provider, vec![sim_a, sim_b]);
    let (_initiator, responder) = pair_ids();
    let t_connected = wire_delay_nanos(SEED, 0);
    let t_accepted = t_connected + wire_delay_nanos(SEED, 1);
    let t_terminate = t_accepted + 1;
    let t_deliver = t_connected + PAYLOAD_HOP_NANOS;
    assert!(t_deliver > t_terminate, "payload hop must land after terminate so the Deliver stays on the heap");

    world.run_until_horizon(t_terminate);
    let leftover = HeapLogKind::Deliver { conn: responder, data_len: 4 };
    assert!(
        world.heap_contents().iter().any(|e| e.kind == leftover),
        "Deliver must still sit on the live heap after terminate: {:?}",
        world.heap_contents()
    );
    assert!(
        !world.heap_log().iter().any(|e| e.kind == leftover),
        "Deliver must not have popped before the leftover-heap check: {:?}",
        world.heap_log()
    );
    assert_eq!(*received.lock(), None, "recv stage must still be pending at terminate");

    world.run_until_horizon(t_deliver);
    assert!(
        world.heap_log().iter().any(|e| e.kind == leftover),
        "Deliver must pop and log after pending was dropped: {:?}",
        world.heap_log()
    );
    assert!(
        !world.heap_contents().iter().any(|e| e.kind == leftover),
        "Deliver must have left the live heap: {:?}",
        world.heap_contents()
    );
    assert_eq!(*received.lock(), None, "gone recv stage must not be resumed");
}

/// Two graphs park the same child Name (`recv`). Graph 0 terminate must drop only
/// that graph's pending. Graph 1's later Deliver still resumes. A name-only gone
/// set would clear graph 1 too — the one-node terminate test cannot catch this.
/// Live `heap_contents` / `heap_log`, not a sorted-only proof.
#[tokio::test]
async fn test_terminate_does_not_drop_other_graph_same_stage_name() {
    const PAYLOAD_HOP_NANOS: u64 = 50_000_000;
    const WAIT_BEFORE_TERMINATE_NANOS: u64 = 20_000_000;
    let handle = tokio::runtime::Handle::current();
    let provider = Arc::new(WorldConnectionProvider::with_payload_delay(SEED, PAYLOAD_HOP_NANOS, PAYLOAD_HOP_NANOS));
    let listener0: SocketAddr = "127.0.0.1:9810".parse().unwrap();
    let listener1: SocketAddr = "127.0.0.1:9820".parse().unwrap();
    let received0 = observed::<Vec<u8>>();
    let received0_a = received0.clone();
    let received1 = observed::<Vec<u8>>();
    let received1_a = received1.clone();
    let trace0 = TraceBuffer::new_shared(100, 1_000_000);
    let trace1 = TraceBuffer::new_shared(100, 1_000_000);

    let mut graph0 = SimulationBuilder::default().with_trace_buffer(trace0).with_eval_strategy(Fifo);
    graph0.resources().put::<ConnectionsResource>(provider.clone());
    let stage0 = graph0.stage("parent", move |_state: (), _unit: (), eff| {
        let received0_a = received0_a.clone();
        async move {
            let net = Network::new(&eff);
            net.listen(listener0).await.unwrap();
            let (_peer, conn) = net.accept(listener0).await.unwrap();
            let child = eff
                .stage("recv", move |(), conn: ConnectionId, eff| {
                    let received0_a = received0_a.clone();
                    async move {
                        let net = Network::new(&eff);
                        let bytes = net.recv(conn, NonZeroUsize::new(4).unwrap()).await.unwrap();
                        set_observed(&received0_a, bytes.as_ref().to_vec());
                    }
                })
                .await;
            let child = eff.wire_up(child, ()).await;
            eff.send(&child, conn).await;
            eff.wait(Duration::from_nanos(WAIT_BEFORE_TERMINATE_NANOS)).await;
            eff.terminate().await
        }
    });
    let stage0 = graph0.wire_up(stage0, ());
    let mut sim0 = graph0.run(&handle);
    sim0.enqueue_msg(&stage0, [()]);

    let mut graph1 = SimulationBuilder::default().with_trace_buffer(trace1).with_eval_strategy(Fifo);
    graph1.resources().put::<ConnectionsResource>(provider.clone());
    let stage1 = graph1.stage("parent", move |_state: (), _unit: (), eff| {
        let received1_a = received1_a.clone();
        async move {
            let net = Network::new(&eff);
            net.listen(listener1).await.unwrap();
            let (_peer, conn) = net.accept(listener1).await.unwrap();
            let child = eff
                .stage("recv", move |(), conn: ConnectionId, eff| {
                    let received1_a = received1_a.clone();
                    async move {
                        let net = Network::new(&eff);
                        let bytes = net.recv(conn, NonZeroUsize::new(5).unwrap()).await.unwrap();
                        set_observed(&received1_a, bytes.as_ref().to_vec());
                    }
                })
                .await;
            let child = eff.wire_up(child, ()).await;
            eff.send(&child, conn).await;
            eff.wait(Duration::from_secs(3600)).await;
        }
    });
    let stage1 = graph1.wire_up(stage1, ());
    let mut sim1 = graph1.run(&handle);
    sim1.enqueue_msg(&stage1, [()]);

    let mut graph2 = SimulationBuilder::default().with_eval_strategy(Fifo);
    graph2.resources().put::<ConnectionsResource>(provider.clone());
    let stage2 = graph2.stage("sender0", move |_state: (), _unit: (), eff| async move {
        let net = Network::new(&eff);
        let conn = net.connect(peer_addr(listener0), Duration::from_secs(1)).await.unwrap();
        net.send(conn, NonEmptyBytes::try_from(Bytes::from("ping")).unwrap()).await.unwrap();
    });
    let stage2 = graph2.wire_up(stage2, ());
    let mut sim2 = graph2.run(&handle);
    sim2.enqueue_msg(&stage2, [()]);

    let mut graph3 = SimulationBuilder::default().with_eval_strategy(Fifo);
    graph3.resources().put::<ConnectionsResource>(provider.clone());
    let stage3 = graph3.stage("sender1", move |_state: (), _unit: (), eff| async move {
        let net = Network::new(&eff);
        let conn = net.connect(peer_addr(listener1), Duration::from_secs(1)).await.unwrap();
        net.send(conn, NonEmptyBytes::try_from(Bytes::from("hello")).unwrap()).await.unwrap();
    });
    let stage3 = graph3.wire_up(stage3, ());
    let mut sim3 = graph3.run(&handle);
    sim3.enqueue_msg(&stage3, [()]);

    let mut world = WorldLoop::new(provider, vec![sim0, sim1, sim2, sim3]);
    let both_accepted_by = 2 * WIRE_DELAY_MAX_NANOS;
    world.run_until_horizon(both_accepted_by);
    assert!(
        world
            .heap_log()
            .iter()
            .any(|e| matches!(e.kind, HeapLogKind::Accepted { listener, .. } if listener == listener0)),
        "graph 0 must have accepted before terminate: {:?}",
        world.heap_log()
    );
    assert!(
        world
            .heap_log()
            .iter()
            .any(|e| matches!(e.kind, HeapLogKind::Accepted { listener, .. } if listener == listener1)),
        "graph 1 must have accepted before terminate: {:?}",
        world.heap_log()
    );
    let leftover = world
        .heap_contents()
        .into_iter()
        .find(|e| matches!(e.kind, HeapLogKind::Deliver { data_len: 4, .. }))
        .expect("graph 0 Deliver must sit on the live heap after both accepts");
    let live = world
        .heap_contents()
        .into_iter()
        .find(|e| matches!(e.kind, HeapLogKind::Deliver { data_len: 5, .. }))
        .expect("graph 1 Deliver must sit on the live heap after both accepts");
    let t_accepted0 = world
        .heap_log()
        .iter()
        .find(|e| matches!(e.kind, HeapLogKind::Accepted { listener, .. } if listener == listener0))
        .expect("graph 0 Accepted")
        .time_nanos;
    let t_terminate = t_accepted0 + WAIT_BEFORE_TERMINATE_NANOS;
    assert!(
        t_terminate < leftover.time_nanos && t_terminate < live.time_nanos,
        "terminate must land before both Delivers so they stay on the heap: terminate={t_terminate} leftover={} live={}",
        leftover.time_nanos,
        live.time_nanos
    );
    assert_eq!(*received0.lock(), None, "graph 0 recv must still be pending at accept");
    assert_eq!(*received1.lock(), None, "graph 1 recv must still be pending at accept");

    world.run_until_horizon(t_terminate);
    assert!(
        world.heap_contents().iter().any(|e| e.kind == leftover.kind),
        "graph 0 leftover Deliver must still sit on the live heap after terminate: {:?}",
        world.heap_contents()
    );
    assert!(
        world.heap_contents().iter().any(|e| e.kind == live.kind),
        "graph 1 Deliver must still sit on the live heap after graph 0 terminate: {:?}",
        world.heap_contents()
    );
    assert!(
        !world.heap_log().iter().any(|e| e.kind == leftover.kind || e.kind == live.kind),
        "neither Deliver may have popped before the leftover-heap check: {:?}",
        world.heap_log()
    );
    assert_eq!(*received0.lock(), None, "graph 0 recv must still be pending at terminate");
    assert_eq!(*received1.lock(), None, "graph 1 recv must still be pending at terminate");

    world.run_until_horizon(leftover.time_nanos.max(live.time_nanos));
    assert!(
        world.heap_log().iter().any(|e| e.kind == leftover.kind),
        "graph 0 leftover Deliver must pop and log without resuming the gone stage: {:?}",
        world.heap_log()
    );
    assert!(
        world.heap_log().iter().any(|e| e.kind == live.kind),
        "graph 1 Deliver must pop and log: {:?}",
        world.heap_log()
    );
    assert!(
        !world.heap_contents().iter().any(|e| e.kind == leftover.kind || e.kind == live.kind),
        "both Delivers must have left the live heap: {:?}",
        world.heap_contents()
    );
    assert_eq!(*received0.lock(), None, "gone graph 0 recv must not be resumed");
    assert_eq!(
        received1.lock().as_deref(),
        Some(b"hello".as_ref()),
        "graph 1 recv with the same Name must still resume"
    );
}

/// A Wait must resume from next_wakeup even when the network heap is empty.
#[tokio::test]
async fn test_wait_resumes_without_heap_event() {
    let handle = tokio::runtime::Handle::current();
    let provider = provider();
    let done = observed::<bool>();
    let done_a = done.clone();

    let mut graph = SimulationBuilder::default().with_eval_strategy(Fifo);
    graph.resources().put::<ConnectionsResource>(provider.clone());
    let stage = graph.stage("waiter", move |_state: (), _unit: (), eff| {
        let done_a = done_a.clone();
        async move {
            eff.wait(Duration::from_nanos(1_000)).await;
            set_observed(&done_a, true);
        }
    });
    let stage = graph.wire_up(stage, ());
    let mut sim = graph.run(&handle);
    sim.enqueue_msg(&stage, [()]);

    let mut world = WorldLoop::new(provider, vec![sim]);
    world.run_to_completion();
    assert_eq!(*done.lock(), Some(true));
}

/// A Wait on a graph with a non-zero global epoch offset must still resume.
/// Instant comparison is `sim_elapsed + offset`. Waking with a zero-offset
/// max_time misses that Wait and reschedules the Sleeping wake forever, so a
/// later Deliver never pops.
#[tokio::test]
async fn test_sleeping_wake_uses_graph_epoch_offset() {
    let handle = tokio::runtime::Handle::current();
    let provider = provider();
    let done = observed::<bool>();
    let done_a = done.clone();
    let epoch_offset = Duration::from_secs(70_419_600);
    let wait_at = 100_000_000;
    let deliver_at = 200_000_000;
    let hop_conn = ConnectionId::initial();
    provider.schedule_event_at(deliver_at, NetworkEvent::Deliver { conn: hop_conn, data: Bytes::from_static(b"x") });

    let mut graph = SimulationBuilder::default().with_eval_strategy(Fifo).with_global_epoch_offset(epoch_offset);
    graph.resources().put::<ConnectionsResource>(provider.clone());
    let stage = graph.stage("waiter", move |_state: (), _unit: (), eff| {
        let done_a = done_a.clone();
        async move {
            eff.wait(Duration::from_nanos(wait_at)).await;
            set_observed(&done_a, true);
        }
    });
    let stage = graph.wire_up(stage, ());
    let mut sim = graph.run(&handle);
    sim.enqueue_msg(&stage, [()]);

    let mut world = WorldLoop::new(provider, vec![sim]);
    world.run_until_horizon(wait_at.saturating_sub(1));
    assert_eq!(*done.lock(), None, "Wait must not complete before its wakeup");
    assert_eq!(world.peek_next_event_time(), Some(wait_at));

    world.run_until_horizon(deliver_at);
    assert_eq!(*done.lock(), Some(true), "Sleeping wait must resume using the graph epoch offset");
    assert!(
        world.heap_log().iter().any(|e| e.kind == HeapLogKind::Deliver { conn: hop_conn, data_len: 1 }),
        "later Deliver must pop after the offset Wait wakes"
    );
}

#[tokio::test]
async fn test_listen_same_port_errors() {
    let handle = tokio::runtime::Handle::current();
    let provider = provider();
    let listener_addr: SocketAddr = "127.0.0.1:9500".parse().unwrap();
    let second = observed::<bool>();
    let second_a = second.clone();

    let mut graph = SimulationBuilder::default().with_eval_strategy(Fifo);
    graph.resources().put::<ConnectionsResource>(provider.clone());
    let stage = graph.stage("node", move |_state: (), _unit: (), eff| {
        let second_a = second_a.clone();
        async move {
            let net = Network::new(&eff);
            net.listen(listener_addr).await.unwrap();
            set_observed(&second_a, net.listen(listener_addr).await.is_err());
        }
    });
    let stage = graph.wire_up(stage, ());
    let mut sim = graph.run(&handle);
    sim.enqueue_msg(&stage, [()]);

    let mut world = WorldLoop::new(provider, vec![sim]);
    world.run_to_completion();
    assert_eq!(*second.lock(), Some(true));
}

#[tokio::test]
async fn test_connect_refused_at_attempt_arrival() {
    let _guards = trace_guards();
    let handle = tokio::runtime::Handle::current();
    let provider = provider();
    let trace = TraceBuffer::new_shared(100, 1_000_000);
    let listener_addr: SocketAddr = "127.0.0.1:9600".parse().unwrap();
    let failed = observed::<bool>();
    let failed_b = failed.clone();

    let mut graph = SimulationBuilder::default().with_trace_buffer(trace.clone()).with_eval_strategy(Fifo);
    graph.resources().put::<ConnectionsResource>(provider.clone());
    let stage = graph.stage("node", move |_state: (), _unit: (), eff| {
        let failed_b = failed_b.clone();
        async move {
            let net = Network::new(&eff);
            set_observed(&failed_b, net.connect(peer_addr(listener_addr), Duration::from_secs(1)).await.is_err());
        }
    });
    let stage = graph.wire_up(stage, ());
    let mut sim = graph.run(&handle);
    sim.enqueue_msg(&stage, [()]);

    let mut world = WorldLoop::new(provider, vec![sim]);
    world.run_to_completion();
    assert_eq!(*failed.lock(), Some(true));

    let t_attempt = wire_delay_nanos(SEED, 0);
    assert_ne!(t_attempt, 0);
    assert!((WIRE_DELAY_MIN_NANOS..=WIRE_DELAY_MAX_NANOS).contains(&t_attempt));
    assert_heap_log(
        world.take_heap_log(),
        vec![
            graph_wake(0, 0, 0, GraphWakeReason::Runnable),
            HeapLogEntry {
                sequence: 1,
                time_nanos: t_attempt,
                kind: HeapLogKind::ConnectAttempt { target: listener_addr },
            },
            graph_wake(3, t_attempt, 0, GraphWakeReason::Runnable),
        ],
    );

    assert_trace_match_filter(
        world.graph(0),
        &[
            tm_state("node-1", &()),
            tm_input("node-1", &()),
            tm_resume_unit("node-1"),
            tm_effect("node-1", ConnectEffect { peer: peer_addr(listener_addr), timeout: Duration::from_secs(1) }),
            tm_clock(Duration::from_nanos(t_attempt)),
            tm_resume_external(
                "node-1",
                Err::<ConnectionId, ConnectError>(ConnectError::new(peer_addr(listener_addr), "connection refused")),
            ),
            tm_state("node-1", &()),
        ],
        &[],
    );
}

#[tokio::test]
async fn test_connect_times_out_before_attempt_arrives() {
    let _guards = trace_guards();
    let handle = tokio::runtime::Handle::current();
    let provider = provider();
    let trace = TraceBuffer::new_shared(100, 1_000_000);
    let listener_addr: SocketAddr = "127.0.0.1:9601".parse().unwrap();
    let failed = observed::<bool>();
    let failed_b = failed.clone();
    let timeout = Duration::from_nanos(1);

    let mut graph = SimulationBuilder::default().with_trace_buffer(trace.clone()).with_eval_strategy(Fifo);
    graph.resources().put::<ConnectionsResource>(provider.clone());
    let stage = graph.stage("node", move |_state: (), _unit: (), eff| {
        let failed_b = failed_b.clone();
        async move {
            let net = Network::new(&eff);
            set_observed(&failed_b, net.connect(peer_addr(listener_addr), timeout).await.is_err());
        }
    });
    let stage = graph.wire_up(stage, ());
    let mut sim = graph.run(&handle);
    sim.enqueue_msg(&stage, [()]);

    let mut world = WorldLoop::new(provider, vec![sim]);
    world.run_to_completion();
    assert_eq!(*failed.lock(), Some(true));

    assert_heap_log(
        world.take_heap_log(),
        vec![
            graph_wake(0, 0, 0, GraphWakeReason::Runnable),
            HeapLogEntry { sequence: 2, time_nanos: 1, kind: HeapLogKind::ConnectTimeout { target: listener_addr } },
            graph_wake(3, 1, 0, GraphWakeReason::Runnable),
        ],
    );

    assert_trace_match_filter(
        world.graph(0),
        &[
            tm_state("node-1", &()),
            tm_input("node-1", &()),
            tm_resume_unit("node-1"),
            tm_effect("node-1", ConnectEffect { peer: peer_addr(listener_addr), timeout }),
            tm_clock(timeout),
            tm_resume_external(
                "node-1",
                Err::<ConnectionId, ConnectError>(ConnectError::new(peer_addr(listener_addr), "timed out")),
            ),
            tm_state("node-1", &()),
        ],
        &[],
    );
}

#[tokio::test]
async fn test_connect_timeout_does_not_pair_listener() {
    let handle = tokio::runtime::Handle::current();
    let provider = provider();
    let listener_addr: SocketAddr = "127.0.0.1:9602".parse().unwrap();
    let failed = observed::<bool>();
    let failed_b = failed.clone();
    let timeout = Duration::from_nanos(1);

    let mut graph_a = SimulationBuilder::default().with_eval_strategy(Fifo);
    graph_a.resources().put::<ConnectionsResource>(provider.clone());
    let stage_a = graph_a.stage("node_a", move |_state: (), _unit: (), eff| async move {
        let net = Network::new(&eff);
        net.listen(listener_addr).await.unwrap();
        let _ = net.accept(listener_addr).await.unwrap();
    });
    let stage_a = graph_a.wire_up(stage_a, ());
    let mut sim_a = graph_a.run(&handle);
    sim_a.enqueue_msg(&stage_a, [()]);

    let mut graph_b = SimulationBuilder::default().with_eval_strategy(Fifo);
    graph_b.resources().put::<ConnectionsResource>(provider.clone());
    let stage_b = graph_b.stage("node_b", move |_state: (), _unit: (), eff| {
        let failed_b = failed_b.clone();
        async move {
            let net = Network::new(&eff);
            set_observed(&failed_b, net.connect(peer_addr(listener_addr), timeout).await.is_err());
        }
    });
    let stage_b = graph_b.wire_up(stage_b, ());
    let mut sim_b = graph_b.run(&handle);
    sim_b.enqueue_msg(&stage_b, [()]);

    let mut world = WorldLoop::new(provider, vec![sim_a, sim_b]);
    world.run_until_horizon(WIRE_DELAY_MAX_NANOS);
    assert_eq!(*failed.lock(), Some(true));
    assert!(
        world
            .heap_log()
            .iter()
            .all(|e| !matches!(e.kind, HeapLogKind::Accepted { .. } | HeapLogKind::ConnectAttempt { .. })),
        "timed-out connect must not SYN or pair: {:?}",
        world.heap_log()
    );
}

#[tokio::test]
async fn test_peer_disconnect_closes_a_live_pair() {
    let handle = tokio::runtime::Handle::current();
    let provider = provider();
    let listener_addr: SocketAddr = "127.0.0.1:9603".parse().unwrap();
    let disconnect_at = 10_000_000;

    let mut graph_a = SimulationBuilder::default().with_eval_strategy(Fifo);
    graph_a.resources().put::<ConnectionsResource>(provider.clone());
    let stage_a = graph_a.stage("node_a", move |_state: (), _unit: (), eff| async move {
        let net = Network::new(&eff);
        net.listen(listener_addr).await.unwrap();
        let _ = net.accept(listener_addr).await;
    });
    let stage_a = graph_a.wire_up(stage_a, ());
    let mut sim_a = graph_a.run(&handle);
    sim_a.enqueue_msg(&stage_a, [()]);

    let mut graph_b = SimulationBuilder::default().with_eval_strategy(Fifo);
    graph_b.resources().put::<ConnectionsResource>(provider.clone());
    let stage_b = graph_b.stage("node_b", move |_state: (), _unit: (), eff| async move {
        let net = Network::new(&eff);
        let conn = net.connect(peer_addr(listener_addr), Duration::from_secs(1)).await.unwrap();
        let _ = net.send(conn, NonEmptyBytes::try_from(Bytes::from("x")).unwrap()).await;
    });
    let stage_b = graph_b.wire_up(stage_b, ());
    let mut sim_b = graph_b.run(&handle);
    sim_b.enqueue_msg(&stage_b, [()]);

    provider.schedule_peer_disconnects(1, disconnect_at, disconnect_at, None);
    let mut world = WorldLoop::new(provider, vec![sim_a, sim_b]);
    world.run_until_horizon(disconnect_at.saturating_add(WIRE_DELAY_MAX_NANOS));

    let log = world.heap_log();
    assert!(
        log.iter().any(|e| e.kind == HeapLogKind::PeerDisconnect && e.time_nanos == disconnect_at),
        "fault hop must pop at the scheduled time: {log:?}"
    );
    assert!(log.iter().any(|e| matches!(e.kind, HeapLogKind::Close { .. })), "fault must close a live pair: {log:?}");
}

#[tokio::test]
async fn test_send_recv_on_closed_peer_reset() {
    let handle = tokio::runtime::Handle::current();
    let provider = provider();
    let listener_addr: SocketAddr = "127.0.0.1:9700".parse().unwrap();
    let send_err = observed::<bool>();
    let recv_err = observed::<bool>();
    let send_err_b = send_err.clone();
    let recv_err_a = recv_err.clone();

    let mut graph_a = SimulationBuilder::default().with_eval_strategy(Fifo);
    graph_a.resources().put::<ConnectionsResource>(provider.clone());
    let stage_a = graph_a.stage("node_a", move |_state: (), _unit: (), eff| {
        let recv_err_a = recv_err_a.clone();
        async move {
            let net = Network::new(&eff);
            net.listen(listener_addr).await.unwrap();
            let (_peer, conn) = net.accept(listener_addr).await.unwrap();
            set_observed(&recv_err_a, net.recv(conn, NonZeroUsize::new(1).unwrap()).await.is_err());
        }
    });
    let stage_a = graph_a.wire_up(stage_a, ());
    let mut sim_a = graph_a.run(&handle);
    sim_a.enqueue_msg(&stage_a, [()]);

    let mut graph_b = SimulationBuilder::default().with_eval_strategy(Fifo);
    graph_b.resources().put::<ConnectionsResource>(provider.clone());
    let stage_b = graph_b.stage("node_b", move |_state: (), _unit: (), eff| {
        let send_err_b = send_err_b.clone();
        async move {
            let net = Network::new(&eff);
            let conn = net.connect(peer_addr(listener_addr), Duration::from_secs(1)).await.unwrap();
            net.close(conn).await.unwrap();
            eff.wait(Duration::from_nanos(1)).await;
            set_observed(
                &send_err_b,
                net.send(conn, NonEmptyBytes::try_from(Bytes::from("x")).unwrap()).await.is_err(),
            );
        }
    });
    let stage_b = graph_b.wire_up(stage_b, ());
    let mut sim_b = graph_b.run(&handle);
    sim_b.enqueue_msg(&stage_b, [()]);

    let mut world = WorldLoop::new(provider, vec![sim_a, sim_b]);
    world.run_to_completion();
    assert_eq!(*recv_err.lock(), Some(true));
    assert_eq!(*send_err.lock(), Some(true));
}

#[tokio::test]
async fn test_latency_is_one_to_five_ms() {
    let handle = tokio::runtime::Handle::current();
    let provider = provider();
    let listener_addr: SocketAddr = "127.0.0.1:9800".parse().unwrap();
    let received = observed::<Vec<u8>>();
    let received_a = received.clone();

    let mut graph_a = SimulationBuilder::default().with_eval_strategy(Fifo);
    graph_a.resources().put::<ConnectionsResource>(provider.clone());
    let stage_a = graph_a.stage("node_a", move |_state: (), _unit: (), eff| {
        let received_a = received_a.clone();
        async move {
            let net = Network::new(&eff);
            net.listen(listener_addr).await.unwrap();
            let (_peer, conn) = net.accept(listener_addr).await.unwrap();
            let bytes = net.recv(conn, NonZeroUsize::new(1).unwrap()).await.unwrap();
            set_observed(&received_a, bytes.as_ref().to_vec());
        }
    });
    let stage_a = graph_a.wire_up(stage_a, ());
    let mut sim_a = graph_a.run(&handle);
    sim_a.enqueue_msg(&stage_a, [()]);

    let mut graph_b = SimulationBuilder::default().with_eval_strategy(Fifo);
    graph_b.resources().put::<ConnectionsResource>(provider.clone());
    let stage_b = graph_b.stage("node_b", move |_state: (), _unit: (), eff| async move {
        let net = Network::new(&eff);
        let conn = net.connect(peer_addr(listener_addr), Duration::from_secs(1)).await.unwrap();
        net.send(conn, NonEmptyBytes::try_from(Bytes::from("z")).unwrap()).await.unwrap();
    });
    let stage_b = graph_b.wire_up(stage_b, ());
    let mut sim_b = graph_b.run(&handle);
    sim_b.enqueue_msg(&stage_b, [()]);

    let mut world = WorldLoop::new(provider, vec![sim_a, sim_b]);
    world.run_to_completion();
    assert_eq!(received.lock().as_deref(), Some(b"z".as_ref()));

    let hop = world
        .take_heap_log()
        .into_iter()
        .find(|e| matches!(e.kind, HeapLogKind::ConnectAttempt { .. } | HeapLogKind::Deliver { .. }));
    let hop = hop.expect("ConnectAttempt or Deliver on the heap log");
    assert_ne!(hop.time_nanos, 0, "wire hop must be delayed");
    assert!(
        (WIRE_DELAY_MIN_NANOS..=WIRE_DELAY_MAX_NANOS).contains(&hop.time_nanos),
        "wire hop time {} not in 1ms..=5ms",
        hop.time_nanos
    );
}

#[test]
fn test_honest_payload_cap_is_five_preprod_slots() {
    let slot = PREPROD_ERA_HISTORY.current_era_summary().params.slot_length;
    assert_eq!(slot, Duration::from_secs(1), "preprod slot length");
    assert_eq!(
        Duration::from_nanos(HONEST_PAYLOAD_DELAY_MAX_NANOS),
        slot * u32::try_from(HONEST_PAYLOAD_DELAY_SLOTS).expect("slot budget fits u32"),
    );
}

/// Seeded long-tail samples stay in the 1–5ms hop for the majority, with at least one
/// sample orders of magnitude later. A uniform draw over `[1ms, 5s]` fails this.
#[test]
fn test_long_tail_payload_delay_is_not_uniform_over_five_slots() {
    const N: u64 = 4096;
    let samples: Vec<u64> = (0..N).map(|index| long_tail_payload_delay_nanos(SEED, index)).collect();
    let short = samples.iter().filter(|d| (WIRE_DELAY_MIN_NANOS..=WIRE_DELAY_MAX_NANOS).contains(d)).count();
    let long =
        samples.iter().filter(|d| (LONG_TAIL_PAYLOAD_MIN_NANOS..=HONEST_PAYLOAD_DELAY_MAX_NANOS).contains(d)).count();
    assert!(
        short * 10 > samples.len(),
        "almost all samples must stay in the 1–5ms hop, not a uniform [1ms, 5s] draw; short={short}/{}",
        samples.len()
    );
    assert!(
        long >= 1 && (long as u64) * LONG_TAIL_PAYLOAD_EVERY / 2 < N,
        "long-tail must be rare (~1/{LONG_TAIL_PAYLOAD_EVERY}), got long={long}/{}",
        samples.len()
    );
    assert!(
        samples.iter().all(|d| {
            (WIRE_DELAY_MIN_NANOS..=WIRE_DELAY_MAX_NANOS).contains(d)
                || (LONG_TAIL_PAYLOAD_MIN_NANOS..=HONEST_PAYLOAD_DELAY_MAX_NANOS).contains(d)
        }),
        "every sample must be a short hop or a long-tail hop within the per-send cap: {samples:?}"
    );
    let again: Vec<u64> = (0..N).map(|index| long_tail_payload_delay_nanos(SEED, index)).collect();
    assert_eq!(samples, again, "long-tail samples must be deterministic for a seed");
}

/// Two honest payloads sent at the same instant — one short hop, one long-tail — sit on
/// the one physical heap at those times. `PAYLOAD_SEED` draws that pair. Pop the short first;
/// the long one stays on the heap. A sorted `assert_heap_log` cannot hide a missing late payload.
#[test]
fn test_short_and_long_tail_payloads_sit_on_one_heap() {
    const PAYLOAD_SEED: u64 = 126;
    let d0 = long_tail_payload_delay_nanos(PAYLOAD_SEED, 0);
    let d1 = long_tail_payload_delay_nanos(PAYLOAD_SEED, 1);
    let short_band = WIRE_DELAY_MIN_NANOS..=WIRE_DELAY_MAX_NANOS;
    let long_band = LONG_TAIL_PAYLOAD_MIN_NANOS..=HONEST_PAYLOAD_DELAY_MAX_NANOS;
    assert!(
        (short_band.contains(&d0) && long_band.contains(&d1)) || (long_band.contains(&d0) && short_band.contains(&d1)),
        "seed {PAYLOAD_SEED} must draw one short hop and one long-tail payload, got {d0} and {d1}"
    );

    let provider = Arc::new(WorldConnectionProvider::with_long_tail_payload_delay(PAYLOAD_SEED));
    let (conn0, conn1) = pair_ids();
    provider.schedule_payload(NetworkEvent::Deliver { conn: conn0, data: Bytes::from_static(b"p0") });
    provider.schedule_payload(NetworkEvent::Deliver { conn: conn1, data: Bytes::from_static(b"p1") });

    let first = HeapLogEntry { sequence: 0, time_nanos: d0, kind: HeapLogKind::Deliver { conn: conn0, data_len: 2 } };
    let second = HeapLogEntry { sequence: 1, time_nanos: d1, kind: HeapLogKind::Deliver { conn: conn1, data_len: 2 } };
    let (early, late) = if d0 < d1 { (first, second) } else { (second, first) };

    let mut world = WorldLoop::new(provider, vec![]);
    assert_eq!(
        world.heap_contents(),
        vec![early, late],
        "both payloads must already sit on the one heap at their sampled times"
    );
    assert!(short_band.contains(&early.time_nanos), "the earlier heap entry must be the short hop");
    assert!(long_band.contains(&late.time_nanos), "the later heap entry must be the long-tail payload");

    world.run_until_horizon(early.time_nanos);
    assert_eq!(world.take_heap_log(), vec![early], "short payload must pop first, not a sorted log of both");
    assert_eq!(world.heap_contents(), vec![late], "long-tail payload must still be on the heap");

    world.run_until_horizon(late.time_nanos);
    assert_eq!(world.take_heap_log(), vec![late]);
    assert!(world.heap_contents().is_empty(), "both payloads must have been popped");
}

/// Two payloads on one destination stay in send order. A long-tail first hop holds the
/// short second hop (TCP FIFO). Different connections may still pass each other.
#[test]
fn test_same_conn_payloads_stay_in_send_order() {
    let provider = Arc::new(WorldConnectionProvider::with_long_tail_payload_delay(SEED));
    let (conn, _) = pair_ids();
    provider.schedule_payload(NetworkEvent::Deliver { conn, data: Bytes::from_static(b"first") });
    provider.schedule_payload(NetworkEvent::Deliver { conn, data: Bytes::from_static(b"second") });

    let first = HeapLogEntry { sequence: 0, time_nanos: 0, kind: HeapLogKind::Deliver { conn, data_len: 5 } };
    let second = HeapLogEntry { sequence: 1, time_nanos: 0, kind: HeapLogKind::Deliver { conn, data_len: 6 } };
    let mut world = WorldLoop::new(provider, vec![]);
    let on_heap = world.heap_contents();
    assert_eq!(on_heap.len(), 2, "both same-conn payloads sit on the heap");
    assert_eq!(on_heap[0].kind, first.kind);
    assert_eq!(on_heap[1].kind, second.kind);
    assert!(on_heap[0].time_nanos <= on_heap[1].time_nanos, "later send cannot arrive earlier on the same conn");
    assert!(on_heap[0].sequence < on_heap[1].sequence);

    world.run_until_horizon(on_heap[1].time_nanos);
    let log = world.take_heap_log();
    assert_eq!(log[0].kind, first.kind);
    assert_eq!(log[1].kind, second.kind);
    assert!(log[0].time_nanos <= log[1].time_nanos);
    assert!(world.heap_contents().is_empty());
}

/// A ready-now graph wake and a `NetworkEvent` at the same nanos are one heap.
///
/// `Deliver` is scheduled first (seq 0). The graph is then placed on that same heap
/// as `GraphWake` (seq 1). Both at t=0. The loop must pop `Deliver` then the graph —
/// not run the graph to park and only then the hop. `heap_contents` is checked
/// **before** the loop so both items are first-class heap entries, not a Vec scan.
#[tokio::test]
async fn test_equal_time_graph_wake_and_network_event_are_one_heap() {
    let _guards = trace_guards();
    let handle = tokio::runtime::Handle::current();
    let provider = provider();
    let trace = TraceBuffer::new_shared(100, 1_000_000);
    let hop_conn = ConnectionId::initial();
    provider.schedule_event_at(0, NetworkEvent::Deliver { conn: hop_conn, data: Bytes::from_static(b"x") });

    let ran = observed::<bool>();
    let ran_a = ran.clone();
    let mut graph = SimulationBuilder::default().with_trace_buffer(trace.clone()).with_eval_strategy(Fifo);
    graph.resources().put::<ConnectionsResource>(provider.clone());
    let stage = graph.stage("ready", move |_state: (), _unit: (), _eff| {
        let ran_a = ran_a.clone();
        async move {
            set_observed(&ran_a, true);
        }
    });
    let stage = graph.wire_up(stage, ());
    let mut sim = graph.run(&handle);
    sim.enqueue_msg(&stage, [()]);

    let mut world = WorldLoop::new(provider, vec![sim]);
    assert_eq!(*ran.lock(), None, "graph must not run until its heap entry is popped");
    let deliver =
        HeapLogEntry { sequence: 0, time_nanos: 0, kind: HeapLogKind::Deliver { conn: hop_conn, data_len: 1 } };
    let wake = graph_wake(1, 0, 0, GraphWakeReason::Runnable);
    assert_eq!(
        world.heap_contents(),
        vec![deliver, wake],
        "graph wake and NetworkEvent must already share one heap at the same nanos"
    );
    assert_eq!(deliver.time_nanos, wake.time_nanos);
    assert!(deliver.sequence < wake.sequence);

    world.run_to_completion();
    assert_eq!(*ran.lock(), Some(true));

    let log = world.take_heap_log();
    assert_eq!(&log[..2], &[deliver, wake], "pop order must follow (time, sequence), not all-graphs-then-hop");

    assert_trace_match_filter(
        world.graph(0),
        &[tm_state("ready-1", &()), tm_input("ready-1", &()), tm_resume_unit("ready-1"), tm_state("ready-1", &())],
        &[],
    );
}

/// A sleeping graph wake and a `Deliver` at the same nanos sit on one heap before either pops.
/// Pop order follows `(time, sequence)` — the hop is not deferred until the graph parks.
#[tokio::test]
async fn test_equal_time_wait_and_deliver_share_one_heap() {
    let _guards = trace_guards();
    let handle = tokio::runtime::Handle::current();
    let provider = provider();
    let trace = TraceBuffer::new_shared(100, 1_000_000);
    let hop_time = 2_000_000;
    let hop_conn = ConnectionId::initial();
    provider.schedule_event_at(hop_time, NetworkEvent::Deliver { conn: hop_conn, data: Bytes::from_static(b"x") });

    let woke = observed::<bool>();
    let woke_a = woke.clone();
    let mut graph = SimulationBuilder::default().with_trace_buffer(trace.clone()).with_eval_strategy(Fifo);
    graph.resources().put::<ConnectionsResource>(provider.clone());
    let stage = graph.stage("waiter", move |_state: (), _unit: (), eff| {
        let woke_a = woke_a.clone();
        async move {
            eff.wait(Duration::from_nanos(hop_time)).await;
            set_observed(&woke_a, true);
        }
    });
    let stage = graph.wire_up(stage, ());
    let mut sim = graph.run(&handle);
    sim.enqueue_msg(&stage, [()]);

    let mut world = WorldLoop::new(provider, vec![sim]);
    world.run_until_horizon(hop_time.saturating_sub(1));
    assert_eq!(*woke.lock(), None, "Wait must not complete before the shared timestamp");

    let deliver =
        HeapLogEntry { sequence: 0, time_nanos: hop_time, kind: HeapLogKind::Deliver { conn: hop_conn, data_len: 1 } };
    let at_hop: Vec<_> = world.heap_contents().into_iter().filter(|e| e.time_nanos == hop_time).collect();
    assert_eq!(at_hop.len(), 2, "Deliver and GraphWake must both be on the heap at {hop_time}: {at_hop:?}");
    assert_eq!(at_hop[0], deliver);
    assert!(
        matches!(at_hop[1].kind, HeapLogKind::GraphWake { reason: GraphWakeReason::Sleeping, graph: 0 }),
        "expected Sleeping graph wake: {at_hop:?}"
    );
    assert_eq!(at_hop[0].time_nanos, at_hop[1].time_nanos);
    assert!(at_hop[0].sequence < at_hop[1].sequence);

    world.run_to_completion();
    assert_eq!(*woke.lock(), Some(true));
    let popped: Vec<_> = world.take_heap_log().into_iter().filter(|e| e.time_nanos == hop_time).collect();
    assert_eq!(popped, at_hop, "pop order at the shared timestamp must match heap (time, sequence)");

    let wait = Duration::from_nanos(hop_time);
    assert_trace_match_filter(
        world.graph(0),
        &[
            tm_state("waiter-1", &()),
            tm_input("waiter-1", &()),
            tm_resume_unit("waiter-1"),
            TraceEntry::suspend(Effect::Wait { at_stage: Name::from("waiter-1"), duration: wait }).into(),
            tm_clock(wait),
            TraceEntry::resume("waiter-1", StageResponse::WaitResponse(Instant::at_offset(wait, Duration::ZERO)))
                .into(),
            tm_state("waiter-1", &()),
        ],
        &[],
    );
}

/// A sleeping graph is scheduled at `next_wakeup` and does not run before that time
/// while an earlier Deliver can complete.
#[tokio::test]
async fn test_sleeping_graph_does_not_run_before_earlier_deliver() {
    let handle = tokio::runtime::Handle::current();
    let provider = provider();
    let deliver_at = 1_000_000;
    let wake_at = 10_000_000;
    let hop_conn = ConnectionId::initial();
    provider.schedule_event_at(deliver_at, NetworkEvent::Deliver { conn: hop_conn, data: Bytes::from_static(b"x") });

    let done = observed::<bool>();
    let done_a = done.clone();
    let mut graph = SimulationBuilder::default().with_eval_strategy(Fifo);
    graph.resources().put::<ConnectionsResource>(provider.clone());
    let stage = graph.stage("late", move |_state: (), _unit: (), eff| {
        let done_a = done_a.clone();
        async move {
            eff.wait(Duration::from_nanos(wake_at)).await;
            set_observed(&done_a, true);
        }
    });
    let stage = graph.wire_up(stage, ());
    let mut sim = graph.run(&handle);
    sim.enqueue_msg(&stage, [()]);

    let mut world = WorldLoop::new(provider, vec![sim]);
    world.run_until_horizon(deliver_at);
    assert_eq!(*done.lock(), None, "graph must still be sleeping when the earlier Deliver pops");
    let log = world.heap_log();
    assert!(log.iter().any(|e| e.kind == HeapLogKind::Deliver { conn: hop_conn, data_len: 1 }));
    assert!(!log.iter().any(|e| {
        matches!(e.kind, HeapLogKind::GraphWake { reason: GraphWakeReason::Sleeping, .. }) && e.time_nanos == wake_at
    }));
    assert_eq!(world.peek_next_event_time(), Some(wake_at));

    world.run_to_completion();
    assert_eq!(*done.lock(), Some(true));
    let log = by_time_seq(world.take_heap_log());
    let deliver_seq =
        log.iter().find(|e| e.kind == HeapLogKind::Deliver { conn: hop_conn, data_len: 1 }).expect("Deliver").sequence;
    let wake_seq = log
        .iter()
        .find(|e| {
            matches!(e.kind, HeapLogKind::GraphWake { reason: GraphWakeReason::Sleeping, .. })
                && e.time_nanos == wake_at
        })
        .expect("Sleeping graph wake")
        .sequence;
    assert!(deliver_seq < wake_seq, "earlier Deliver must have a lower sequence than the later graph wake");
}
