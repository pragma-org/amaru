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
//! Synthetic header trees. Topology, peer sharing, delay, interleavings, and
//! randomized peer disconnects. Validation effects may be stubbed. `cmp_tip`
//! here is not Conway ledger acceptance. See EDR-011 "World tests: generated
//! vs recorded chains".
//!
//! Every run prints `seed=0x…`. Replay with `AMARU_TEST_SEED=<that value>`.

use std::{env::var, net::SocketAddr, sync::Arc, time::Duration};

use amaru_consensus::{
    effects::{GenerateRandomSeed, ValidateBlockEffect, ValidateHeaderEffect},
    stages::select_chain::cmp_tip,
};
use amaru_kernel::{
    BlockHeight, Hash, IsHeader, NetworkPoint, PREPROD_ERA_HISTORY, PREPROD_GLOBAL_PARAMETERS, Peer, Point, Slot,
    any_headers_chain_with_root, cardano::network_block::make_encoded_block, utils::tests::run_strategy_with_seed,
};
use amaru_metrics::LedgerMetrics;
use amaru_ouroboros::{
    BaseReadChainStore, ConnectionsResource, Nonces, WriteChainStore, in_memory_chain_store::InMemoryChainStore,
};
use amaru_protocols::store_effects::{ResourceHeaderStore, ResourceParameters};
use amaru_pure_stage::{simulation::running::OverrideResult, trace_buffer::TraceBuffer};
use tokio::runtime::Handle;

use super::{
    HONEST_PAYLOAD_DELAY_MAX_NANOS, HeapLogEntry, HeapLogKind, WIRE_DELAY_MAX_NANOS, WorldConnectionProvider,
    WorldLoop, build_injector, build_injector_peer, build_world_node,
    support::{
        derive_seed, draw_test_seed, fragment_trace_guards, peer_saw_roll_forward, peer_trace, seed_bytes, test_seeds,
        tm_chainsync_roll_forward, tm_chainsync_roll_forward_of, tm_validate_header,
    },
};
use crate::tests::configuration::NodeTestConfig;

const TAG_NODE: u64 = 1;
const TAG_INJECTOR: u64 = 100;
const TAG_INJECTOR_PEER: u64 = 101;
const TAG_PEER_SEL: u64 = 200;

fn provider(seed: u64) -> Arc<WorldConnectionProvider> {
    Arc::new(WorldConnectionProvider::new(seed))
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
    let seed = draw_test_seed();
    eprintln!("world boot_connect_exchange seed={seed:#x}");
    let _guards = fragment_trace_guards();
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let handle = runtime.handle().clone();
    let provider = Arc::new(WorldConnectionProvider::with_long_tail_payload_delay(seed));

    let conway_start_slot = Slot::from(68_774_400);
    let root_point = NetworkPoint::Specific(conway_start_slot, Hash::new([0u8; 32]));
    let headers =
        run_strategy_with_seed(seed, any_headers_chain_with_root(2, root_point.with_height(BlockHeight::from(0))));

    let listen_a = "127.0.0.1:9311";
    let listen_b = "127.0.0.1:9310";
    let peer_a = Peer::new(listen_a);

    let node_a = NodeTestConfig::default()
        .with_no_upstream_peers()
        .with_listen_address(listen_a)
        .with_seed(derive_seed(seed, TAG_NODE))
        .with_trace_buffer(TraceBuffer::new_shared(10_000, 8_000_000))
        .with_validated_blocks(headers);
    let node_b = NodeTestConfig::default()
        .with_upstream_peer(peer_a)
        .with_listen_address(listen_b)
        .with_seed(derive_seed(seed, TAG_NODE + 1))
        .with_trace_buffer(TraceBuffer::new_shared(10_000, 8_000_000));

    let connections: ConnectionsResource = provider.clone();
    let mut sim_a = build_world_node(&node_a, connections.clone(), &handle).expect("node A");
    let mut sim_b = build_world_node(&node_b, connections, &handle).expect("node B");
    stub_peer_selection_seed(&mut sim_a, derive_seed(seed, TAG_PEER_SEL));
    stub_peer_selection_seed(&mut sim_b, derive_seed(seed, TAG_PEER_SEL + 1));

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
    assert!(
        log.iter().any(|e| matches!(e.kind, HeapLogKind::ConnectAttempt { .. })),
        "nodes must connect; seed={seed:#x} heap={log:?}"
    );
    assert!(
        log.iter().any(|e| matches!(e.kind, HeapLogKind::Accepted { .. })),
        "nodes must accept; seed={seed:#x} heap={log:?}"
    );
    assert!(
        log.iter().any(|e| matches!(e.kind, HeapLogKind::SendAck { .. })),
        "nodes must send; seed={seed:#x} heap={log:?}"
    );
    assert!(
        log.iter().any(|e| matches!(e.kind, HeapLogKind::Deliver { .. })),
        "nodes must deliver; seed={seed:#x} heap={log:?}"
    );

    let header_on_wire = world.graphs().iter().any(|graph| {
        graph
            .trace_buffer()
            .lock()
            .hydrate_without_timestamps()
            .iter()
            .any(|entry| tm_chainsync_roll_forward() == *entry || tm_validate_header() == *entry)
    });
    assert!(
        header_on_wire,
        "expected a typed chainsync RollForward or ValidateHeaderEffect; seed={seed:#x} heap={log:?}"
    );
}

fn stub_peer_selection_seed(sim: &mut amaru_pure_stage::simulation::running::SimulationRunning, seed: u64) {
    let bytes = seed_bytes(seed);
    sim.override_external_effect::<GenerateRandomSeed>(usize::MAX, move |_| OverrideResult::handled(bytes));
}

fn injector_linear_store(n: usize, seed: u64) -> (Arc<InMemoryChainStore>, Vec<amaru_kernel::Header>) {
    let conway_start_slot = Slot::from(68_774_400);
    let root_point = NetworkPoint::Specific(conway_start_slot, Hash::new([0u8; 32]));
    let headers =
        run_strategy_with_seed(seed, any_headers_chain_with_root(n, root_point.with_height(BlockHeight::from(0))));
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
/// Seeded disconnects stay sparse relative to the fragment; their schedule is redrawn until
/// at least one adjacent pair sits inside one reconnect delay. Each inventory hash is a
/// world-heap Reveal, paced by the injector's default mailbox.
const P_JOIN_NODES: usize = 5;
const P_JOIN_FRAGMENT: usize = 100;
/// Payload hop ~10ms ± 2ms. Handshake hops stay 1–5ms.
const P_JOIN_REALISTIC_MIN_NANOS: u64 = 8_000_000;
const P_JOIN_REALISTIC_MAX_NANOS: u64 = 12_000_000;
const P_JOIN_REALISTIC_HORIZON_NANOS: u64 = 60_000_000_000;
const P_JOIN_CHAOS_HORIZON_NANOS: u64 = 180_000_000_000;
const P_JOIN_DISCONNECTS: u32 = P_JOIN_FRAGMENT as u32 / 5;
/// First disconnect after handshakes exist (connect hops are 1–5ms).
const P_JOIN_DISCONNECT_EARLIEST_NANOS: u64 = 100_000_000;
/// Manager outbound reconnect delay (`ManagerConfig::reconnect_delay`).
const P_JOIN_RECONNECT_DELAY_NANOS: u64 = 2_000_000_000;
/// Leave the reconnect delay plus slack after the last drop.
const P_JOIN_DISCONNECT_TAIL_NANOS: u64 = 2_500_000_000;
const P_JOIN_RUNS: u32 = 50;

/// Realistic link delay: finish catch-up quickly on a near-constant hop.
#[test]
fn test_p_join_quiescent_chain_realistic() {
    run_p_join_repeats("realistic", PJoinDelay::Realistic, P_JOIN_REALISTIC_HORIZON_NANOS, 9500);
}

/// Homogeneous long-tail hops: same distribution on every link; allow a couple of minutes.
#[test]
fn test_p_join_quiescent_chain_chaos() {
    run_p_join_repeats("chaos", PJoinDelay::Chaos, P_JOIN_CHAOS_HORIZON_NANOS, 9600);
}

enum PJoinDelay {
    Realistic,
    Chaos,
}

fn run_p_join_repeats(label: &str, delay: PJoinDelay, horizon_nanos: u64, base_port: u16) {
    let _guards = fragment_trace_guards();
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let handle = runtime.handle().clone();
    let runs = if var("GITHUB_ACTIONS").as_deref() == Ok("true") { P_JOIN_RUNS } else { P_JOIN_RUNS / 10 };
    let seeds = test_seeds(runs);
    let n = seeds.len();
    for (i, seed) in seeds.into_iter().enumerate() {
        eprintln!("p-join {label} run={}/{n} seed={seed:#x}", i + 1);
        let provider = match delay {
            PJoinDelay::Realistic => Arc::new(WorldConnectionProvider::with_payload_delay(
                seed,
                P_JOIN_REALISTIC_MIN_NANOS,
                P_JOIN_REALISTIC_MAX_NANOS,
            )),
            PJoinDelay::Chaos => Arc::new(WorldConnectionProvider::with_long_tail_payload_delay(seed)),
        };
        run_p_join_quiescent_chain(label, provider, horizon_nanos, base_port, seed, &handle);
    }
}

fn run_p_join_quiescent_chain(
    label: &str,
    provider: Arc<WorldConnectionProvider>,
    horizon_nanos: u64,
    base_port: u16,
    seed: u64,
    handle: &Handle,
) {
    use std::cmp::Ordering;

    let injector_addr: SocketAddr = format!("127.0.0.1:{base_port}").parse().unwrap();
    let node_addrs: Vec<String> =
        (0..P_JOIN_NODES).map(|i| format!("127.0.0.1:{}", base_port + 11 + i as u16)).collect();

    let (store, headers) = injector_linear_store(P_JOIN_FRAGMENT, seed);
    let head = headers.last().expect("fragment HEAD").clone();
    let source: Arc<dyn BaseReadChainStore> = store;
    let connections: ConnectionsResource = provider.clone();

    let (injector, shared) =
        build_injector(source, connections.clone(), injector_addr, derive_seed(seed, TAG_INJECTOR), handle)
            .expect("injector");

    let mut graphs = vec![injector];
    for (i, listen) in node_addrs.iter().enumerate() {
        let upstream = if i == 0 { Peer::new(&injector_addr.to_string()) } else { Peer::new(&node_addrs[i - 1]) };
        let node = NodeTestConfig::default()
            .with_upstream_peer(upstream)
            .with_listen_address(listen)
            .with_seed(derive_seed(seed, TAG_NODE + i as u64))
            .with_trace_buffer(TraceBuffer::new_shared(20_000, 16_000_000))
            // Common ancestor so FindIntersect is not Origin-vs-a-parent-hash the node does not have.
            .with_validated_blocks(vec![headers[0].clone()]);
        let mut sim = build_world_node(&node, connections.clone(), handle).expect("production node");
        // Generated chain: stub validation (EDR-011).
        sim.override_external_effect::<ValidateHeaderEffect>(usize::MAX, |_| {
            OverrideResult::handled(Ok(Nonces::for_tests()))
        });
        sim.override_external_effect::<ValidateBlockEffect>(usize::MAX, |_| {
            OverrideResult::handled(Ok(Ok(LedgerMetrics::default())))
        });
        stub_peer_selection_seed(&mut sim, derive_seed(seed, TAG_PEER_SEL + i as u64));
        graphs.push(sim);
    }

    let disconnect_latest =
        horizon_nanos.saturating_sub(P_JOIN_DISCONNECT_TAIL_NANOS).max(P_JOIN_DISCONNECT_EARLIEST_NANOS);
    provider.schedule_peer_disconnects(
        P_JOIN_DISCONNECTS,
        P_JOIN_DISCONNECT_EARLIEST_NANOS,
        disconnect_latest,
        Some(P_JOIN_RECONNECT_DELAY_NANOS),
    );

    let mut world = WorldLoop::new(provider, graphs).with_injector(0, shared);
    world.schedule_reveals(headers.iter().map(IsHeader::hash));
    let head_point = head.point();
    let mut adopted_at = None;
    world.run_until_horizon_with(horizon_nanos, Duration::ZERO, |world| {
        if adopted_at.is_none() && p_join_nodes_adopted_head(world, &head_point) {
            adopted_at = Some((world.now_nanos(), p_join_wire_summary(world.heap_log_ref())));
        }
    });
    print_p_join_summary(label, seed, horizon_nanos, adopted_at, world.heap_log_ref());

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
        "peer sharing must add connections beyond the initial {chain_connects}-hop chain; seed={seed:#x} connects={connects}"
    );
    assert!(
        injector_connects >= 2,
        "at least one node besides the chain head must dial the injector; seed={seed:#x} injector_connects={injector_connects}"
    );
    let reveals = log.iter().filter(|e| matches!(e.kind, HeapLogKind::Reveal { .. })).count();
    assert_eq!(
        reveals, P_JOIN_FRAGMENT,
        "each fragment hash must be a world-heap Reveal; seed={seed:#x} reveals={reveals}"
    );
    let disconnects = log.iter().filter(|e| matches!(e.kind, HeapLogKind::PeerDisconnect)).count();
    assert_eq!(
        disconnects, P_JOIN_DISCONNECTS as usize,
        "expected {P_JOIN_DISCONNECTS} injected peer disconnects; seed={seed:#x}"
    );
    assert!(
        log.iter().any(|e| matches!(e.kind, HeapLogKind::Close { .. })),
        "injected disconnects must close a live pair; seed={seed:#x}"
    );

    for (i, graph) in world.graphs().iter().enumerate().skip(1) {
        let store = graph.resources().get::<ResourceHeaderStore>().expect("node chain store");
        let tip = store.get_best_chain_tip();
        let got = store
            .load_header(&tip.hash())
            .unwrap_or_else(|| panic!("node {i} best tip {tip} has no header after {horizon_nanos}ns; seed={seed:#x}"));
        assert_eq!(
            cmp_tip(Some(&got), Some(&head)),
            Ordering::Equal,
            "node {i} adopted tip {tip} must be cmp_tip-equal to the quiescent HEAD {}; seed={seed:#x}",
            head.point()
        );
        assert_eq!(tip, head.point(), "node {i} best-chain pointer must be the quiescent HEAD; seed={seed:#x}");
    }
}

struct PJoinWireSummary {
    messages: usize,
    bytes: usize,
    connections: usize,
}

fn p_join_nodes_adopted_head(world: &WorldLoop, head: &Point) -> bool {
    world.graphs().iter().skip(1).all(|graph| {
        let store = graph.resources().get::<ResourceHeaderStore>().expect("node chain store");
        store.get_best_chain_tip() == *head
    })
}

fn p_join_wire_summary(log: &[HeapLogEntry]) -> PJoinWireSummary {
    let mut messages = 0;
    let mut bytes = 0;
    let mut connections = 0;
    for entry in log {
        match entry.kind {
            HeapLogKind::Deliver { data_len, .. } => {
                messages += 1;
                bytes += data_len;
            }
            HeapLogKind::Accepted { .. } => connections += 1,
            HeapLogKind::ConnectAttempt { .. }
            | HeapLogKind::ConnectTimeout { .. }
            | HeapLogKind::SendAck { .. }
            | HeapLogKind::Close { .. }
            | HeapLogKind::PeerDisconnect
            | HeapLogKind::Reveal { .. }
            | HeapLogKind::GraphWake { .. } => {}
        }
    }
    PJoinWireSummary { messages, bytes, connections }
}

fn format_sim_nanos(nanos: u64) -> String {
    if nanos >= 1_000_000_000 {
        format!("{:.3}s", nanos as f64 / 1_000_000_000.0)
    } else if nanos >= 1_000_000 {
        format!("{:.3}ms", nanos as f64 / 1_000_000.0)
    } else {
        format!("{nanos}ns")
    }
}

fn print_p_join_summary(
    label: &str,
    seed: u64,
    horizon_nanos: u64,
    adopted_at: Option<(u64, PJoinWireSummary)>,
    log: &[HeapLogEntry],
) {
    let (adopted, summary) = match adopted_at {
        Some((nanos, summary)) => (format_sim_nanos(nanos), summary),
        None => (format!("not by {}", format_sim_nanos(horizon_nanos)), p_join_wire_summary(log)),
    };
    eprintln!(
        "p-join {label} summary seed={seed:#x} adopted_head={adopted} messages={} bytes={} connections={}",
        summary.messages, summary.bytes, summary.connections
    );
}

#[tokio::test]
async fn test_injector_inventory_reaches_world_loop() {
    let seed = draw_test_seed();
    eprintln!("world injector_inventory seed={seed:#x}");
    let handle = tokio::runtime::Handle::current();
    let provider = provider(seed);
    let (store, _) = injector_linear_store(3, seed);
    let listen: SocketAddr = "127.0.0.1:9400".parse().unwrap();
    let source: Arc<dyn BaseReadChainStore> = store;
    let connections: ConnectionsResource = provider.clone();
    let (sim, shared) =
        build_injector(source, connections, listen, derive_seed(seed, TAG_INJECTOR), &handle).expect("injector");

    let mut world = WorldLoop::new(provider, vec![sim]).with_injector(0, shared);
    assert_eq!(world.inventory_len(), 3, "inventory is scanned at injector construction");
    world.run_until_horizon(0);
    world.assert_serving_accept(0);
}

#[tokio::test]
async fn test_injector_empty_store_inventory_is_empty() {
    let seed = draw_test_seed();
    eprintln!("world injector_empty_store seed={seed:#x}");
    let handle = tokio::runtime::Handle::current();
    let provider = provider(seed);
    let store = Arc::new(InMemoryChainStore::new());
    let listen: SocketAddr = "127.0.0.1:9401".parse().unwrap();
    let source: Arc<dyn BaseReadChainStore> = store;
    let connections: ConnectionsResource = provider.clone();
    let (sim, shared) =
        build_injector(source, connections, listen, derive_seed(seed, TAG_INJECTOR), &handle).expect("injector");

    let mut world = WorldLoop::new(provider, vec![sim]).with_injector(0, shared);
    assert_eq!(world.inventory_len(), 0);
    world.run_until_horizon(0);
    world.assert_serving_accept(0);
}

/// Before any reveal a peer must not see later headers. Each `reveal` widens the advertised
/// prefix; ChainSync may RollForward only that prefix.
#[tokio::test]
async fn test_injector_reveal_gates_chainsync() {
    let seed = draw_test_seed();
    eprintln!("world injector_reveal_gates seed={seed:#x}");
    let _guards = fragment_trace_guards();
    let handle = tokio::runtime::Handle::current();
    let provider = provider(seed);
    let (store, headers) = injector_linear_store(2, seed);
    let listen: SocketAddr = "127.0.0.1:9402".parse().unwrap();
    let source: Arc<dyn BaseReadChainStore> = store;
    let connections: ConnectionsResource = provider.clone();
    let (injector, shared) =
        build_injector(source, connections.clone(), listen, derive_seed(seed, TAG_INJECTOR), &handle)
            .expect("injector");
    let peer =
        build_injector_peer(connections, listen, derive_seed(seed, TAG_INJECTOR_PEER), &handle).expect("injector peer");

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
