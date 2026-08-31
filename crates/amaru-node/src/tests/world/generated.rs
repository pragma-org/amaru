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

//! Generated-chain world tests.
//!
//! Synthetic header trees. Topology, peer sharing, delay, and interleavings.
//! Validation effects may be stubbed. `cmp_tip` here is not Conway ledger
//! acceptance. See EDR-011 "World tests: generated vs recorded chains".

use std::{net::SocketAddr, sync::Arc};

use amaru_consensus::{
    effects::{ValidateBlockEffect, ValidateHeaderEffect},
    stages::select_chain::cmp_tip,
};
use amaru_kernel::{
    BlockHeight, Hash, IsHeader, NetworkPoint, PREPROD_ERA_HISTORY, PREPROD_GLOBAL_PARAMETERS, Peer, Slot,
    any_headers_chain_with_root, cardano::network_block::make_encoded_block, utils::tests::run_strategy,
};
use amaru_metrics::LedgerMetrics;
use amaru_ouroboros::{
    BaseReadChainStore, ConnectionsResource, Nonces, WriteChainStore, in_memory_chain_store::InMemoryChainStore,
};
use amaru_protocols::store_effects::{ResourceHeaderStore, ResourceParameters};
use amaru_pure_stage::{simulation::running::OverrideResult, trace_buffer::TraceBuffer};

use super::{
    HONEST_PAYLOAD_DELAY_MAX_NANOS, HeapLogKind, WIRE_DELAY_MAX_NANOS, WorldConnectionProvider, WorldLoop,
    build_injector, build_injector_peer, build_world_node,
    support::{
        SEED, fragment_trace_guards, peer_saw_roll_forward, peer_trace, tm_chainsync_roll_forward,
        tm_chainsync_roll_forward_of, tm_validate_header,
    },
};
use crate::tests::configuration::NodeTestConfig;

fn provider() -> Arc<WorldConnectionProvider> {
    Arc::new(WorldConnectionProvider::new(SEED))
}

/// Two production-shaped nodes (`build_node` × SimulationBuilder × SimulationRunning)
/// over one WorldConnectionProvider, driven only by WorldLoop.
///
/// Proves they boot, connect, and put at least one header on the wire (typed
/// chainsync `RollForward` or `ValidateHeaderEffect`). Does not claim tip equality
/// and does not load a preprod fragment. `k` stays at the production value.
/// Long-tail payload delay is a world setting, not a theorem. Horizon only runs
/// far enough for sampled Deliveries to pop; it is not a Praos deadline.
///
/// Not `#[tokio::test]`: production graphs issue DurationDist::Zero effects whose `run()`
/// may be Pending on the first poll, and SimulationRunning then `Handle::block_on`s them.
/// That panics inside an existing Tokio context. WorldLoop is therefore synchronous.
#[test]
fn test_world_owns_production_nodes_boot_connect_exchange() {
    let _guards = fragment_trace_guards();
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let handle = runtime.handle().clone();
    let provider = Arc::new(WorldConnectionProvider::with_long_tail_payload_delay(SEED));

    let conway_start_slot = Slot::from(68_774_400);
    let root_point = NetworkPoint::Specific(conway_start_slot, Hash::new([0u8; 32]));
    let headers = run_strategy(any_headers_chain_with_root(2, root_point.with_height(BlockHeight::from(0))));

    let listen_a = "127.0.0.1:9311";
    let listen_b = "127.0.0.1:9310";
    let peer_a = Peer::new(listen_a);

    let node_a = NodeTestConfig::default()
        .with_no_upstream_peers()
        .with_listen_address(listen_a)
        .with_seed(11)
        .with_trace_buffer(TraceBuffer::new_shared(10_000, 8_000_000))
        .with_validated_blocks(headers);
    let node_b = NodeTestConfig::default()
        .with_upstream_peer(peer_a)
        .with_listen_address(listen_b)
        .with_seed(12)
        .with_trace_buffer(TraceBuffer::new_shared(10_000, 8_000_000));

    let connections: ConnectionsResource = provider.clone();
    let sim_a = build_world_node(&node_a, connections.clone(), &handle).expect("node A");
    let sim_b = build_world_node(&node_b, connections, &handle).expect("node B");

    let mut world = WorldLoop::new(provider, vec![sim_a, sim_b]);
    // World coverage so a sampled long-tail Deliver can pop. Not a Praos deadline.
    world.run_until_horizon(HONEST_PAYLOAD_DELAY_MAX_NANOS.saturating_add(WIRE_DELAY_MAX_NANOS));

    for graph in world.graphs() {
        let params = graph.resources().get::<ResourceParameters>().expect("production GlobalParameters");
        assert_eq!(
            params.consensus_security_param, PREPROD_GLOBAL_PARAMETERS.consensus_security_param,
            "world nodes must keep production k"
        );
        assert_eq!(params.consensus_security_param, 2160);
    }

    let log = world.heap_log();
    assert!(log.iter().any(|e| matches!(e.kind, HeapLogKind::ConnectAttempt { .. })), "nodes must connect: {log:?}");
    assert!(log.iter().any(|e| matches!(e.kind, HeapLogKind::Accepted { .. })), "nodes must accept: {log:?}");
    assert!(log.iter().any(|e| matches!(e.kind, HeapLogKind::SendAck { .. })), "nodes must send: {log:?}");
    assert!(log.iter().any(|e| matches!(e.kind, HeapLogKind::Deliver { .. })), "nodes must deliver: {log:?}");

    let header_on_wire = world.graphs().iter().any(|graph| {
        graph
            .trace_buffer()
            .lock()
            .hydrate_without_timestamps()
            .iter()
            .any(|entry| tm_chainsync_roll_forward() == *entry || tm_validate_header() == *entry)
    });
    assert!(header_on_wire, "expected a typed chainsync RollForward or ValidateHeaderEffect; heap={log:?}");
}

fn injector_linear_store(n: usize) -> (Arc<InMemoryChainStore>, Vec<amaru_kernel::Header>) {
    let conway_start_slot = Slot::from(68_774_400);
    let root_point = NetworkPoint::Specific(conway_start_slot, Hash::new([0u8; 32]));
    let headers = run_strategy(any_headers_chain_with_root(n, root_point.with_height(BlockHeight::from(0))));
    let store = Arc::new(InMemoryChainStore::new());
    store.set_anchor_point(&headers[0].point()).unwrap();
    for header in &headers {
        store.store_header(header).unwrap();
        store.store_block(&header.hash(), &make_encoded_block(header, &PREPROD_ERA_HISTORY)).unwrap();
        store.roll_forward_chain(&header.point()).unwrap();
    }
    (store, headers)
}

/// Five production nodes in a line, head dialing the injector. P-join on a quiescent
/// fragment: the injector reveals the whole inventory up front (no live minting).
///
/// Peer sharing must add connections beyond the initial chain. Delay and horizon are knobs.
const P_JOIN_NODES: usize = 5;
const P_JOIN_FRAGMENT: usize = 8;
/// Payload hop ~10ms ± 2ms. Handshake hops stay 1–5ms.
const P_JOIN_REALISTIC_MIN_NANOS: u64 = 8_000_000;
const P_JOIN_REALISTIC_MAX_NANOS: u64 = 12_000_000;
const P_JOIN_REALISTIC_HORIZON_NANOS: u64 = 5_000_000_000;
const P_JOIN_CHAOS_HORIZON_NANOS: u64 = 120_000_000_000;

/// Realistic link delay: finish catch-up quickly on a near-constant hop.
#[test]
fn test_p_join_quiescent_chain_realistic() {
    let provider = Arc::new(WorldConnectionProvider::with_payload_delay(
        SEED,
        P_JOIN_REALISTIC_MIN_NANOS,
        P_JOIN_REALISTIC_MAX_NANOS,
    ));
    run_p_join_quiescent_chain(provider, P_JOIN_REALISTIC_HORIZON_NANOS, 9500);
}

/// Homogeneous long-tail hops: same distribution on every link; allow a couple of minutes.
#[test]
fn test_p_join_quiescent_chain_chaos() {
    let provider = Arc::new(WorldConnectionProvider::with_long_tail_payload_delay(SEED));
    run_p_join_quiescent_chain(provider, P_JOIN_CHAOS_HORIZON_NANOS, 9600);
}

fn run_p_join_quiescent_chain(provider: Arc<WorldConnectionProvider>, horizon_nanos: u64, base_port: u16) {
    use std::cmp::Ordering;

    let _guards = fragment_trace_guards();
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let handle = runtime.handle().clone();

    let injector_addr: SocketAddr = format!("127.0.0.1:{base_port}").parse().unwrap();
    let node_addrs: Vec<String> =
        (0..P_JOIN_NODES).map(|i| format!("127.0.0.1:{}", base_port + 11 + i as u16)).collect();

    let (store, headers) = injector_linear_store(P_JOIN_FRAGMENT);
    let head = headers.last().expect("fragment HEAD").clone();
    let source: Arc<dyn BaseReadChainStore> = store;
    let connections: ConnectionsResource = provider.clone();

    let (injector, shared) = build_injector(source, connections.clone(), injector_addr, &handle).expect("injector");

    let mut graphs = vec![injector];
    for (i, listen) in node_addrs.iter().enumerate() {
        let upstream = if i == 0 { Peer::new(&injector_addr.to_string()) } else { Peer::new(&node_addrs[i - 1]) };
        let node = NodeTestConfig::default()
            .with_upstream_peer(upstream)
            .with_listen_address(listen)
            .with_seed(31 + i as u64)
            .with_trace_buffer(TraceBuffer::new_shared(20_000, 16_000_000))
            // Common ancestor so FindIntersect is not Origin-vs-a-parent-hash the node does not have.
            .with_validated_blocks(vec![headers[0].clone()]);
        let mut sim = build_world_node(&node, connections.clone(), &handle).expect("production node");
        // Generated chain: stub validation (EDR-011).
        sim.override_external_effect::<ValidateHeaderEffect>(usize::MAX, |_| {
            OverrideResult::handled(Ok(Nonces::for_tests()))
        });
        sim.override_external_effect::<ValidateBlockEffect>(usize::MAX, |_| {
            OverrideResult::handled(Ok(Ok(LedgerMetrics::default())))
        });
        graphs.push(sim);
    }

    let mut world = WorldLoop::new(provider, graphs).with_injector(0, shared);
    for header in &headers {
        world.reveal(header.hash()).expect("reveal quiescent fragment");
    }
    world.run_until_horizon(horizon_nanos);

    for graph in world.graphs().iter().skip(1) {
        let params = graph.resources().get::<ResourceParameters>().expect("production GlobalParameters");
        assert_eq!(params.consensus_security_param, PREPROD_GLOBAL_PARAMETERS.consensus_security_param);
        assert_eq!(params.consensus_security_param, 2160, "production k");
    }

    let log = world.heap_log();
    let chain_connects = P_JOIN_NODES;
    let connects = log.iter().filter(|e| matches!(e.kind, HeapLogKind::ConnectAttempt { .. })).count();
    let injector_connects = log
        .iter()
        .filter(|e| matches!(e.kind, HeapLogKind::ConnectAttempt { target } if target == injector_addr))
        .count();
    assert!(
        connects > chain_connects,
        "peer sharing must add connections beyond the initial {chain_connects}-hop chain; connects={connects} heap={log:?}"
    );
    assert!(
        injector_connects >= 2,
        "at least one node besides the chain head must dial the injector; injector_connects={injector_connects} heap={log:?}"
    );

    for (i, graph) in world.graphs().iter().enumerate().skip(1) {
        let store = graph.resources().get::<ResourceHeaderStore>().expect("node chain store");
        let tip = store.get_best_chain_tip();
        let got = store
            .load_header(&tip.hash())
            .unwrap_or_else(|| panic!("node {i} best tip {tip} has no header after {horizon_nanos}ns"));
        assert_eq!(
            cmp_tip(Some(&got), Some(&head)),
            Ordering::Equal,
            "node {i} adopted tip {tip} must be cmp_tip-equal to the quiescent HEAD {}",
            head.point()
        );
        assert_eq!(tip, head.point(), "node {i} best-chain pointer must be the quiescent HEAD");
    }
}

#[tokio::test]
async fn test_injector_inventory_reaches_world_loop() {
    let handle = tokio::runtime::Handle::current();
    let provider = provider();
    let (store, _) = injector_linear_store(3);
    let listen: SocketAddr = "127.0.0.1:9400".parse().unwrap();
    let source: Arc<dyn BaseReadChainStore> = store;
    let connections: ConnectionsResource = provider.clone();
    let (sim, shared) = build_injector(source, connections, listen, &handle).expect("injector");

    let mut world = WorldLoop::new(provider, vec![sim]).with_injector(0, shared);
    assert_eq!(world.inventory_len(), 3, "inventory is scanned at injector construction");
    world.run_until_horizon(0);
    world.assert_serving_accept(0);
}

#[tokio::test]
async fn test_injector_empty_store_inventory_is_empty() {
    let handle = tokio::runtime::Handle::current();
    let provider = provider();
    let store = Arc::new(InMemoryChainStore::new());
    let listen: SocketAddr = "127.0.0.1:9401".parse().unwrap();
    let source: Arc<dyn BaseReadChainStore> = store;
    let connections: ConnectionsResource = provider.clone();
    let (sim, shared) = build_injector(source, connections, listen, &handle).expect("injector");

    let mut world = WorldLoop::new(provider, vec![sim]).with_injector(0, shared);
    assert_eq!(world.inventory_len(), 0);
    world.run_until_horizon(0);
    world.assert_serving_accept(0);
}

/// Before any reveal a peer must not see later headers. Each `reveal` widens the advertised
/// prefix; ChainSync may RollForward only that prefix.
#[tokio::test]
async fn test_injector_reveal_gates_chainsync() {
    let _guards = fragment_trace_guards();
    let handle = tokio::runtime::Handle::current();
    let provider = provider();
    let (store, headers) = injector_linear_store(2);
    let listen: SocketAddr = "127.0.0.1:9402".parse().unwrap();
    let source: Arc<dyn BaseReadChainStore> = store;
    let connections: ConnectionsResource = provider.clone();
    let (injector, shared) = build_injector(source, connections.clone(), listen, &handle).expect("injector");
    let peer = build_injector_peer(connections, listen, &handle).expect("injector peer");

    let mut world = WorldLoop::new(provider, vec![injector, peer]).with_injector(0, shared);
    // Handshake hops are 1–5ms; a few mux frames finish well before 200ms.
    world.run_until_horizon(200_000_000);
    assert_eq!(world.inventory_len(), 2);
    assert!(!peer_saw_roll_forward(&world, 1, &headers[0].hash()), "no header is visible before WorldLoop reveal");
    assert!(!peer_saw_roll_forward(&world, 1, &headers[1].hash()));

    world.reveal(headers[0].hash()).expect("reveal header 1");
    world.run_until_horizon(400_000_000);
    assert!(
        peer_trace(&world, 1).iter().any(|entry| tm_chainsync_roll_forward_of(headers[0].hash()) == *entry),
        "ChainSync may RollForward the first revealed header"
    );
    assert!(
        peer_trace(&world, 1).iter().all(|entry| tm_chainsync_roll_forward_of(headers[1].hash()) != *entry),
        "header 2 stays hidden after reveal 1"
    );

    world.reveal(headers[1].hash()).expect("reveal header 2");
    world.run_until_horizon(600_000_000);
    assert!(
        peer_trace(&world, 1).iter().any(|entry| tm_chainsync_roll_forward_of(headers[1].hash()) == *entry),
        "ChainSync may RollForward the second revealed header"
    );
}
