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

use std::{cmp::Ordering, env::var, net::SocketAddr, num::NonZeroU8, sync::Arc, time::Duration};

use amaru_consensus::{
    effects::{GenerateRandomSeed, ValidateBlockEffect, ValidateHeaderEffect},
    stages::select_chain::cmp_tip,
};
use amaru_kernel::{
    BlockHeight, Hash, Header, IsHeader, NetworkPoint, PREPROD_ERA_HISTORY, PREPROD_GLOBAL_PARAMETERS, Peer, Point,
    Slot, any_headers_chain_with_root, cardano::network_block::make_encoded_block,
    utils::tests::run_strategy_with_seed,
};
use amaru_metrics::LedgerMetrics;
use amaru_ouroboros::{
    BaseReadChainStore, ConnectionsResource, Nonces, WriteChainStore, in_memory_chain_store::InMemoryChainStore,
};
use amaru_protocols::store_effects::{ResourceHeaderStore, ResourceParameters};
use amaru_pure_stage::{
    simulation::{SimulationRunning, running::OverrideResult},
    trace_buffer::TraceBuffer,
};
use tokio::runtime::{Handle, Runtime};

use super::{
    HONEST_PAYLOAD_DELAY_MAX_NANOS, HeapLogEntry, HeapLogKind, InjectorShared, WIRE_DELAY_MAX_NANOS,
    WorldConnectionProvider, WorldLoop, build_injector, build_injector_peer, build_world_node,
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

/// First node listen port is `base + NODE_PORT_OFFSET`.
const NODE_PORT_OFFSET: u16 = 11;

const BLOCKFETCH_FRAGMENT: usize = 6;
const BLOCKFETCH_HORIZON_NANOS: u64 = 5_000_000_000;

fn provider(seed: u64) -> Arc<WorldConnectionProvider> {
    Arc::new(WorldConnectionProvider::new(seed))
}

fn loopback(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

fn node_listen(base: u16, index: usize) -> SocketAddr {
    loopback(base + NODE_PORT_OFFSET + index as u16)
}

fn peer_at(addr: SocketAddr) -> Peer {
    Peer::try_from(addr).expect("world tests use IPv4 loopback")
}

fn conway_root() -> NetworkPoint {
    NetworkPoint::Specific(Slot::from(68_774_400), Hash::new([0u8; 32]))
}

fn generated_headers(n: usize, seed: u64) -> Vec<Header> {
    run_strategy_with_seed(seed, any_headers_chain_with_root(n, conway_root().with_height(BlockHeight::from(0))))
}

fn injector_linear_store(n: usize, seed: u64) -> (Arc<InMemoryChainStore>, Vec<Header>) {
    let headers = generated_headers(n, seed);
    let store = Arc::new(InMemoryChainStore::new());
    store.set_anchor_point(&headers[0].point()).unwrap();
    for header in &headers {
        store.store_header(header).unwrap();
        store.store_block(&header.hash(), &make_encoded_block(header, &PREPROD_ERA_HISTORY)).unwrap();
        store.roll_forward_chain(&header.point()).unwrap();
    }
    (store, headers)
}

fn stub_peer_selection_seed(sim: &mut SimulationRunning, seed: u64) {
    let bytes = seed_bytes(seed);
    sim.override_external_effect::<GenerateRandomSeed>(usize::MAX, move |_| OverrideResult::handled(bytes));
}

fn stub_generated_validation(sim: &mut SimulationRunning) {
    sim.override_external_effect::<ValidateHeaderEffect>(usize::MAX, |_| {
        OverrideResult::handled(Ok(Nonces::for_tests()))
    });
    sim.override_external_effect::<ValidateBlockEffect>(usize::MAX, |_| {
        OverrideResult::handled(Ok(Ok(LedgerMetrics::default())))
    });
}

fn generated_node(seed: u64, index: usize, listen: SocketAddr) -> NodeTestConfig {
    NodeTestConfig::default()
        .with_listen_address(&listen.to_string())
        .with_seed(derive_seed(seed, TAG_NODE + index as u64))
        .with_trace_buffer(TraceBuffer::new_shared(10_000, 8_000_000))
}

fn with_ancestor(config: NodeTestConfig, ancestor: &Header) -> NodeTestConfig {
    config.with_validated_blocks(vec![ancestor.clone()])
}

fn spawn_node(
    config: NodeTestConfig,
    connections: ConnectionsResource,
    handle: &Handle,
    seed: u64,
    index: usize,
    stub_validation: bool,
) -> SimulationRunning {
    let mut sim = build_world_node(&config, connections, handle).expect("production node");
    if stub_validation {
        stub_generated_validation(&mut sim);
    }
    stub_peer_selection_seed(&mut sim, derive_seed(seed, TAG_PEER_SEL + index as u64));
    sim
}

fn spawn_injector(
    store: Arc<InMemoryChainStore>,
    connections: ConnectionsResource,
    listen: SocketAddr,
    seed: u64,
    handle: &Handle,
) -> (SimulationRunning, Arc<InjectorShared>) {
    let source: Arc<dyn BaseReadChainStore> = store;
    build_injector(source, connections, listen, derive_seed(seed, TAG_INJECTOR), handle).expect("injector")
}

fn world_with_injector(
    provider: Arc<WorldConnectionProvider>,
    injector: SimulationRunning,
    shared: Arc<InjectorShared>,
    nodes: Vec<SimulationRunning>,
    headers: &[Header],
) -> WorldLoop {
    let mut graphs = Vec::with_capacity(1 + nodes.len());
    graphs.push(injector);
    graphs.extend(nodes);
    let mut world = WorldLoop::new(provider, graphs).with_injector(0, shared);
    world.schedule_reveals(headers.iter().map(IsHeader::hash));
    world
}

/// Sync tests: production graphs may `Handle::block_on` DurationDist::Zero, which
/// panics inside an existing Tokio context.
struct SyncRun {
    seed: u64,
    handle: Handle,
    provider: Arc<WorldConnectionProvider>,
    _runtime: Runtime,
    _guards: amaru_pure_stage::DeserializerGuards,
}

impl SyncRun {
    fn new(label: &str) -> Self {
        Self::with_provider(label, provider)
    }

    fn with_provider(label: &str, make: impl FnOnce(u64) -> Arc<WorldConnectionProvider>) -> Self {
        let seed = draw_test_seed();
        eprintln!("world {label} seed={seed:#x}");
        let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        let handle = runtime.handle().clone();
        let provider = make(seed);
        Self { seed, handle, provider, _runtime: runtime, _guards: fragment_trace_guards() }
    }

    fn connections(&self) -> ConnectionsResource {
        self.provider.clone()
    }

    fn spawn_catch_up(&self, index: usize, config: NodeTestConfig) -> SimulationRunning {
        spawn_node(config, self.connections(), &self.handle, self.seed, index, true)
    }

    fn spawn_live(&self, index: usize, config: NodeTestConfig) -> SimulationRunning {
        spawn_node(config, self.connections(), &self.handle, self.seed, index, false)
    }

    fn spawn_injector(
        &self,
        store: Arc<InMemoryChainStore>,
        listen: SocketAddr,
    ) -> (SimulationRunning, Arc<InjectorShared>) {
        spawn_injector(store, self.connections(), listen, self.seed, &self.handle)
    }

    fn injector_world(
        &self,
        injector: SimulationRunning,
        shared: Arc<InjectorShared>,
        nodes: Vec<SimulationRunning>,
        headers: &[Header],
    ) -> WorldLoop {
        world_with_injector(self.provider.clone(), injector, shared, nodes, headers)
    }
}

/// Serve-only injector graphs do not install [`ResourceParameters`].
fn assert_production_k(graphs: &[SimulationRunning]) {
    for graph in graphs {
        let params = graph.resources().get::<ResourceParameters>().expect("production GlobalParameters");
        assert_eq!(params.consensus_security_param, PREPROD_GLOBAL_PARAMETERS.consensus_security_param);
        assert_eq!(params.consensus_security_param, 2160, "production k");
    }
}

fn assert_dialed(log: &[HeapLogEntry], target: SocketAddr, seed: u64, what: &str) {
    assert!(
        log.iter().any(|e| matches!(e.kind, HeapLogKind::ConnectAttempt { target: t } if t == target)),
        "{what}; seed={seed:#x} heap={log:?}"
    );
}

fn assert_accepted_at(log: &[HeapLogEntry], listener: SocketAddr, seed: u64, what: &str) {
    assert!(
        log.iter().any(|e| matches!(e.kind, HeapLogKind::Accepted { listener: l, .. } if l == listener)),
        "{what}; seed={seed:#x} heap={log:?}"
    );
}

fn assert_not_dialed(log: &[HeapLogEntry], target: SocketAddr, seed: u64, what: &str) {
    assert!(
        !log.iter().any(|e| matches!(e.kind, HeapLogKind::ConnectAttempt { target: t } if t == target)),
        "{what}; seed={seed:#x} heap={log:?}"
    );
}

fn assert_adopted_head(world: &WorldLoop, graph: usize, head: &Header, seed: u64, who: &str) {
    let store = world.graphs()[graph].resources().get::<ResourceHeaderStore>().expect("node chain store");
    let tip = store.get_best_chain_tip();
    let got =
        store.load_header(&tip.hash()).unwrap_or_else(|| panic!("{who} best tip {tip} has no header; seed={seed:#x}"));
    assert_eq!(
        cmp_tip(Some(&got), Some(head)),
        Ordering::Equal,
        "{who} adopted tip {tip} must be cmp_tip-equal to HEAD {}; seed={seed:#x}",
        head.point()
    );
    assert_eq!(tip, head.point(), "{who} best-chain pointer must be the generated HEAD; seed={seed:#x}");
}

fn assert_has_bodies(world: &WorldLoop, graph: usize, headers: &[Header], seed: u64, who: &str) {
    let store = world.graphs()[graph].resources().get::<ResourceHeaderStore>().expect("node chain store");
    for header in headers {
        assert!(
            store.has_block(&header.hash()).expect("has_block"),
            "{who} must have fetched body for {}; seed={seed:#x}",
            header.point()
        );
    }
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
    let run = SyncRun::with_provider("boot_connect_exchange", |seed| {
        Arc::new(WorldConnectionProvider::with_long_tail_payload_delay(seed))
    });
    let headers = generated_headers(2, run.seed);
    let listen_a = loopback(9311);
    let listen_b = loopback(9310);

    let node_a = generated_node(run.seed, 0, listen_a).with_no_upstream_peers().with_validated_blocks(headers);
    let node_b = generated_node(run.seed, 1, listen_b).with_upstream_peer(peer_at(listen_a));

    let mut world = WorldLoop::new(run.provider.clone(), vec![run.spawn_live(0, node_a), run.spawn_live(1, node_b)]);
    world.run_until_horizon(HONEST_PAYLOAD_DELAY_MAX_NANOS.saturating_add(WIRE_DELAY_MAX_NANOS));

    assert_production_k(world.graphs());
    let log = world.heap_log();
    let seed = run.seed;
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
    world.stop();
}

/// Injector plus one production node on a generated fragment. Proves BlockFetch
/// lock-step (`N = 1`) delivers bodies; the pipelined sibling is
/// [`test_world_blockfetch_pipelined`].
#[test]
fn test_world_blockfetch_lock_step() {
    run_blockfetch_generated_chain(NonZeroU8::MIN, 9700);
}

/// Same topology as [`test_world_blockfetch_lock_step`], with CIP-0164 pipeline depth 2.
#[test]
fn test_world_blockfetch_pipelined() {
    run_blockfetch_generated_chain(NonZeroU8::new(2).unwrap(), 9720);
}

fn run_blockfetch_generated_chain(n: NonZeroU8, base_port: u16) {
    let run = SyncRun::new(&format!("blockfetch n={}", n.get()));
    let injector_addr = loopback(base_port);
    let node_addr = node_listen(base_port, 0);
    let (store, headers) = injector_linear_store(BLOCKFETCH_FRAGMENT, run.seed);
    let head = headers.last().expect("fragment HEAD").clone();

    let (injector, shared) = run.spawn_injector(store, injector_addr);
    let node = with_ancestor(
        generated_node(run.seed, 0, node_addr).with_upstream_peer(peer_at(injector_addr)).with_blockfetch_pipeline_n(n),
        &headers[0],
    );
    let mut world = run.injector_world(injector, shared, vec![run.spawn_catch_up(0, node)], &headers);
    world.run_until_horizon_on_best_chain_tip(BLOCKFETCH_HORIZON_NANOS, |_| {});

    let log = world.heap_log();
    assert_dialed(&log, injector_addr, run.seed, "node must connect");
    assert_accepted_at(&log, injector_addr, run.seed, "injector must accept");
    assert_adopted_head(&world, 1, &head, run.seed, "node");
    assert_has_bodies(&world, 1, &headers, run.seed, "node");
}

/// Injector → A → B. A dials the injector and B (duplex handshake). B has no
/// static upstreams; it must promote the inbound from A and fetch the fragment
/// over that bearer.
#[test]
fn test_world_duplex_inbound_upstream_syncs_injector_chain() {
    let run = SyncRun::new("duplex_inbound_upstream");
    let injector_addr = loopback(9820);
    let listen_a = node_listen(9820, 0);
    let listen_b = node_listen(9820, 1);
    let (store, headers) = injector_linear_store(BLOCKFETCH_FRAGMENT, run.seed);
    let head = headers.last().expect("fragment HEAD").clone();

    let (injector, shared) = run.spawn_injector(store, injector_addr);
    let node_a = with_ancestor(
        generated_node(run.seed, 0, listen_a)
            .with_upstream_peers(vec![peer_at(injector_addr), peer_at(listen_b)])
            .with_target_upstream_peers(2)
            .with_peer_mix("static~2"),
        &headers[0],
    );
    let node_b = with_ancestor(
        generated_node(run.seed, 1, listen_b)
            .with_no_upstream_peers()
            .with_target_upstream_peers(1)
            .with_peer_mix("inbound~1"),
        &headers[0],
    );
    let mut world = run.injector_world(
        injector,
        shared,
        vec![run.spawn_catch_up(0, node_a), run.spawn_catch_up(1, node_b)],
        &headers,
    );
    world.run_until_horizon_on_best_chain_tip(BLOCKFETCH_HORIZON_NANOS, |_| {});

    let log = world.heap_log();
    assert_dialed(&log, injector_addr, run.seed, "A must dial the injector");
    assert_dialed(&log, listen_b, run.seed, "A must dial B");
    assert_accepted_at(&log, listen_b, run.seed, "B must accept A");
    assert_not_dialed(&log, listen_a, run.seed, "B must not dial A");
    assert_adopted_head(&world, 2, &head, run.seed, "node B");
    assert_has_bodies(&world, 2, &headers, run.seed, "node B");
}

/// Five production nodes in a line, head dialing the injector. P-join on a quiescent
/// fragment: the injector reveals the whole inventory up front (no live minting).
///
/// Peer sharing must add connections beyond the initial chain. Delay and horizon are knobs.
/// Production share delay is 300s, longer than these horizons, so the nodes use
/// [`P_JOIN_SHARE_INITIAL_DELAY`]. Duplex inbound Using plus skip-links among the line can
/// fill the production upstream target of 3 before a share reply names the injector, so
/// each node targets [`P_JOIN_NODES`] Using slots (one left for the injector). Seeded
/// disconnects stay sparse relative to the fragment; their schedule is redrawn until at
/// least one adjacent pair sits inside one reconnect delay. Each inventory hash is a
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
/// First peer-sharing request after outbound handshake. Must be well below the horizon
/// (production default is 300s).
const P_JOIN_SHARE_INITIAL_DELAY: Duration = Duration::from_millis(100);

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
    let injector_addr = loopback(base_port);
    let node_addrs: Vec<SocketAddr> = (0..P_JOIN_NODES).map(|i| node_listen(base_port, i)).collect();
    let (store, headers) = injector_linear_store(P_JOIN_FRAGMENT, seed);
    let head = headers.last().expect("fragment HEAD").clone();
    let connections: ConnectionsResource = provider.clone();

    let (injector, shared) = spawn_injector(store, connections.clone(), injector_addr, seed, handle);
    let nodes: Vec<_> = node_addrs
        .iter()
        .enumerate()
        .map(|(i, listen)| {
            let upstream = if i == 0 { peer_at(injector_addr) } else { peer_at(node_addrs[i - 1]) };
            let node = with_ancestor(
                generated_node(seed, i, *listen)
                    .with_upstream_peer(upstream)
                    .with_target_upstream_peers(P_JOIN_NODES)
                    .with_share_request_initial_delay(P_JOIN_SHARE_INITIAL_DELAY)
                    .with_trace_buffer(TraceBuffer::new_shared(20_000, 16_000_000)),
                &headers[0],
            );
            spawn_node(node, connections.clone(), handle, seed, i, true)
        })
        .collect();

    let disconnect_latest =
        horizon_nanos.saturating_sub(P_JOIN_DISCONNECT_TAIL_NANOS).max(P_JOIN_DISCONNECT_EARLIEST_NANOS);
    provider.schedule_peer_disconnects(
        P_JOIN_DISCONNECTS,
        P_JOIN_DISCONNECT_EARLIEST_NANOS,
        disconnect_latest,
        Some(P_JOIN_RECONNECT_DELAY_NANOS),
    );

    let mut world = world_with_injector(provider, injector, shared, nodes, &headers);
    let head_point = head.point();
    let mut adopted_at = None;
    world.run_until_horizon_on_best_chain_tip(horizon_nanos, |world| {
        if adopted_at.is_some() {
            return;
        }
        if p_join_nodes_adopted_head(world, &head_point) {
            adopted_at = Some((world.now_nanos(), p_join_wire_summary(world.heap_log_ref())));
        }
    });
    print_p_join_summary(label, seed, horizon_nanos, adopted_at, world.heap_log_ref());

    assert_production_k(&world.graphs()[1..]);

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

    for i in 1..=P_JOIN_NODES {
        assert_adopted_head(&world, i, &head, seed, &format!("node {i}"));
    }
    world.stop();
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

fn tokio_injector_world(
    seed: u64,
    store: Arc<InMemoryChainStore>,
    listen: SocketAddr,
    handle: &Handle,
) -> (WorldLoop, Arc<WorldConnectionProvider>) {
    let provider = provider(seed);
    let (sim, shared) = spawn_injector(store, provider.clone(), listen, seed, handle);
    (WorldLoop::new(provider.clone(), vec![sim]).with_injector(0, shared), provider)
}

#[tokio::test]
async fn test_injector_inventory_reaches_world_loop() {
    let seed = draw_test_seed();
    eprintln!("world injector_inventory seed={seed:#x}");
    let handle = tokio::runtime::Handle::current();
    let (store, _) = injector_linear_store(3, seed);
    let (mut world, _) = tokio_injector_world(seed, store, loopback(9400), &handle);
    assert_eq!(world.inventory_len(), 3, "inventory is scanned at injector construction");
    world.run_until_horizon(0);
    world.assert_serving_accept(0);
}

#[tokio::test]
async fn test_injector_empty_store_inventory_is_empty() {
    let seed = draw_test_seed();
    eprintln!("world injector_empty_store seed={seed:#x}");
    let handle = tokio::runtime::Handle::current();
    let (mut world, _) = tokio_injector_world(seed, Arc::new(InMemoryChainStore::new()), loopback(9401), &handle);
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
    let (store, headers) = injector_linear_store(2, seed);
    let listen = loopback(9402);
    let provider = provider(seed);
    let (injector, shared) = spawn_injector(store, provider.clone(), listen, seed, &handle);
    let peer = build_injector_peer(provider.clone(), listen, derive_seed(seed, TAG_INJECTOR_PEER), &handle)
        .expect("injector peer");

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
