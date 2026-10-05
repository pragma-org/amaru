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

use std::{
    cmp::Ordering,
    env::var,
    net::SocketAddr,
    num::NonZeroU8,
    sync::{Arc, Mutex},
    time::Duration,
};

use amaru_consensus::{
    effects::{GenerateRandomSeed, ValidateBlockEffect, ValidateHeaderEffect},
    performance::{FetchPeerSet, SelectPeersForFetchEffect},
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
use amaru_protocols::{
    manager::ManagerMessage,
    store_effects::{ResourceHeaderStore, ResourceParameters},
};
use amaru_pure_stage::{
    StageRef,
    simulation::{SimulationRunning, running::OverrideResult},
    trace_buffer::TraceBuffer,
};
use tokio::runtime::{Handle, Runtime};
use tracing::field::Visit;
use tracing_subscriber::{Layer, layer::SubscriberExt, registry};

use super::{
    HONEST_PAYLOAD_DELAY_MAX_NANOS, HeapLogEntry, HeapLogKind, InjectorShared, WIRE_DELAY_MAX_NANOS,
    WorldConnectionProvider, WorldLoop, build_injector, build_injector_peer, build_injector_with_mailbox,
    build_world_node,
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
        Self::new_seeded(label, None)
    }

    fn new_seeded(label: &str, seed: Option<u64>) -> Self {
        Self::with_provider_seed(label, seed, provider)
    }

    fn with_provider_seed(
        label: &str,
        seed: Option<u64>,
        make: impl FnOnce(u64) -> Arc<WorldConnectionProvider>,
    ) -> Self {
        let seed = seed.unwrap_or_else(draw_test_seed);
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

/// Handshake accept and the peer's first mini-protocol segments share one simulated
/// instant under this seed (`unknown protocol 32770` before handlers were registered).
const BUNCHED_HANDSHAKE_SEED: u64 = 0x838e_6961_2d7f_14fd;

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
    run_boot_connect_exchange(None);
}

#[test]
fn test_world_owns_production_nodes_boot_connect_exchange_bunched_handshake() {
    run_boot_connect_exchange(Some(BUNCHED_HANDSHAKE_SEED));
}

fn run_boot_connect_exchange(seed: Option<u64>) {
    let run = SyncRun::with_provider_seed("boot_connect_exchange", seed, |seed| {
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

/// A chain store that already holds headers and bodies, with a stale `valid=false` on a block
/// after the ledger tip (a false reject from an earlier run). Startup must clear that flag and
/// re-apply the stored chain; otherwise candidate search skips the invalid block and its
/// descendants and the node sits idle.
#[test]
fn test_world_revalidates_stored_invalid_block_on_startup() {
    let run = SyncRun::new("revalidate_stored_invalid");
    let listen = loopback(9740);
    let headers = generated_headers(6, run.seed);
    let head = headers.last().expect("chain HEAD").clone();
    let rejected = headers[1].clone();

    let config = generated_node(run.seed, 0, listen).with_no_upstream_peers().with_validated_blocks(headers.clone());
    config.chain_store.set_block_valid(&rejected.hash(), false).unwrap();
    assert_eq!(
        config.chain_store.load_header_with_validity(&rejected.hash()).and_then(|(_, v)| v),
        Some(false),
        "precondition: the stored invalid flag is set before build_node"
    );

    let mut world = WorldLoop::new(run.provider.clone(), vec![run.spawn_catch_up(0, config)]);
    world.run_until_horizon(BLOCKFETCH_HORIZON_NANOS);

    assert_adopted_head(&world, 0, &head, run.seed, "node");
    let rejected_validity = {
        let store = world.graphs()[0].resources().get::<ResourceHeaderStore>().expect("node chain store");
        store.load_header_with_validity(&rejected.hash()).and_then(|(_, v)| v)
    };
    assert_eq!(
        rejected_validity,
        Some(true),
        "the previously rejected block must be re-validated; seed={:#x}",
        run.seed
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

/// Same burst as [`BUNCHED_HANDSHAKE_SEED`], on the duplex inbound from A to B.
/// B stayed on the ancestor (five blocks short of the fragment head).
const DUPLEX_BUNCHED_HANDSHAKE_SEED: u64 = 0x73e4_56d4_83e1_783d;

/// Injector → A → B. A dials the injector and B (duplex handshake). B has no
/// static upstreams; it must promote the inbound from A and fetch the fragment
/// over that bearer.
#[test]
fn test_world_duplex_inbound_upstream_syncs_injector_chain() {
    run_duplex_inbound_upstream(None);
}

#[test]
fn test_world_duplex_inbound_upstream_syncs_injector_chain_bunched_handshake() {
    run_duplex_inbound_upstream(Some(DUPLEX_BUNCHED_HANDSHAKE_SEED));
}

fn run_duplex_inbound_upstream(seed: Option<u64>) {
    let run = SyncRun::new_seeded("duplex_inbound_upstream", seed);
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
            | HeapLogKind::StalledReader { .. }
            | HeapLogKind::SilentResponder { .. }
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

/// Production bulk mailbox. Existing world tests keep 10000; admission tests use this.
const ADMISSION_MAILBOX: usize = 10;

/// Reveals wait while the injector mailbox is at capacity. The loop reads
/// [`SimulationRunning::mailbox_size`], so a mailbox of 10 must hold the next reveal back.
#[tokio::test]
async fn test_reveal_pacing_stops_at_mailbox_capacity() {
    assert_eq!(ADMISSION_MAILBOX, amaru_pure_stage::DEFAULT_MAILBOX_SIZE);
    let seed = draw_test_seed();
    eprintln!("world reveal_pacing seed={seed:#x}");
    let handle = Handle::current();
    let (store, headers) = injector_linear_store(4, seed);
    let provider = provider(seed);
    let (mut sim, shared) =
        build_injector_with_mailbox(store, provider.clone(), loopback(9410), seed, ADMISSION_MAILBOX, &handle)
            .expect("injector");
    assert_eq!(sim.mailbox_size(), ADMISSION_MAILBOX);
    let manager = shared.manager();
    let point = shared.reveal_through(headers[0].hash()).expect("header is in the inventory");
    while sim.mailbox_len(&manager) < sim.mailbox_size() {
        sim.enqueue_msg(&manager, [ManagerMessage::new_tip(point)]);
    }
    assert_eq!(sim.mailbox_len(&manager), ADMISSION_MAILBOX);

    // WorldLoop::new admits one queued message via receive_inputs. The stage is then
    // runnable, so a further enqueue stays in the mailbox and fills that slot again.
    let mut world = WorldLoop::new(provider, vec![sim]).with_injector(0, shared);
    assert_eq!(world.graph(0).mailbox_len(&manager), ADMISSION_MAILBOX - 1);
    world.reveal(headers[1].hash()).expect("refill the admitted slot");
    assert_eq!(world.graph(0).mailbox_len(&manager), ADMISSION_MAILBOX);
    world.schedule_reveals(headers.iter().map(IsHeader::hash));
    assert!(
        world.heap_contents().iter().all(|entry| !matches!(entry.kind, HeapLogKind::Reveal { .. })),
        "a full mailbox of {ADMISSION_MAILBOX} must not take another reveal: {:?}",
        world.heap_contents()
    );
}

const ADVERSARIAL_PEERS: usize = 3;
/// Longer than one block-fetch batch (`MAX_MISSING_BLOCKS_PER_BATCH` is 25) so a stuck
/// fan-out leaves a later range unfetched.
const ADVERSARIAL_FRAGMENT: usize = 32;
/// Same bound as the block-fetch world tests. The 60s block-fetch agency timeout
/// forces a recv the world loop itself has to complete, so the horizon stays under it.
const ADVERSARIAL_HORIZON_NANOS: u64 = BLOCKFETCH_HORIZON_NANOS;
/// After the handshake (connects land around 3–8ms) and while this fragment's
/// bodies are still outstanding (adoption of the 32-block head is ~50ms).
const ADVERSARIAL_FAULT_AT_NANOS: u64 = 15_000_000;

#[derive(Clone, Copy)]
enum AdversarialFault {
    None,
    StalledReader,
    SilentResponder,
}

struct AdversarialOutcome {
    seed: u64,
    heights: Vec<u64>,
    mailbox_size: usize,
    mailbox_len: usize,
    /// Highest manager mailbox length seen at a best-chain tip, and again at the horizon.
    mailbox_high: usize,
    requested_peers: Vec<String>,
    fault_at: Option<u64>,
    adopted_at: Option<u64>,
    finished_at: u64,
    bad_peer: SocketAddr,
    honest_peers: Vec<SocketAddr>,
    adopted: bool,
    log: Vec<HeapLogEntry>,
}

struct AdversarialSetup<'a> {
    label: &'a str,
    fault: AdversarialFault,
    /// Node bulk mailbox. Injectors stay at the production default.
    mailbox: usize,
    fragment: usize,
    fault_at: u64,
    horizon: u64,
    seed: Option<u64>,
    /// Every fetch batch asks every connection. Later waves keep hitting the stalled peer.
    broadcast: bool,
}

/// N injectors serve one generated fragment. The node dials all of them.
/// `fault` hits the lowest-address injector at `setup.fault_at`.
fn run_adversarial(label: &str, fault: AdversarialFault) -> AdversarialOutcome {
    run_adversarial_with(AdversarialSetup {
        label,
        fault,
        mailbox: ADMISSION_MAILBOX,
        fragment: ADVERSARIAL_FRAGMENT,
        fault_at: ADVERSARIAL_FAULT_AT_NANOS,
        horizon: ADVERSARIAL_HORIZON_NANOS,
        seed: None,
        broadcast: false,
    })
}

fn run_adversarial_with(setup: AdversarialSetup<'_>) -> AdversarialOutcome {
    let AdversarialSetup { label, fault, mailbox, fragment, fault_at, horizon, seed, broadcast } = setup;
    let run = SyncRun::new_seeded(&format!("adversarial {label}"), seed);
    let base = 9900u16;
    let injector_addrs: Vec<SocketAddr> = (0..ADVERSARIAL_PEERS).map(|i| loopback(base + i as u16)).collect();
    let bad_peer = injector_addrs[0];
    let honest_peers = injector_addrs[1..].to_vec();
    let node_addr = node_listen(base, 0);
    let (store, headers) = injector_linear_store(fragment, run.seed);
    let head = headers.last().expect("fragment head").clone();

    let mut graphs = Vec::new();
    let mut shared0 = None;
    for (index, addr) in injector_addrs.iter().copied().enumerate() {
        let source: Arc<dyn BaseReadChainStore> = store.clone();
        let (mut sim, shared) = build_injector(
            source,
            run.connections(),
            addr,
            derive_seed(run.seed, TAG_INJECTOR + index as u64),
            &run.handle,
        )
        .expect("injector");
        let point = shared.reveal_through(head.hash()).expect("head is in the inventory");
        sim.enqueue_msg(shared.manager(), [ManagerMessage::new_tip(point)]);
        if index == 0 {
            shared0 = Some(shared);
        }
        graphs.push(sim);
    }
    let peers: Vec<Peer> = injector_addrs.iter().copied().map(peer_at).collect();
    let node = with_ancestor(
        generated_node(run.seed, 0, node_addr).with_upstream_peers(peers).with_mailbox_size(mailbox),
        &headers[0],
    );
    let node_idx = graphs.len();
    let mut node_sim = run.spawn_catch_up(0, node);
    if broadcast {
        // Cold-start shape on every batch: the manager offers the range to each connection,
        // including one that has stopped reading.
        node_sim.override_external_effect::<SelectPeersForFetchEffect>(usize::MAX, |_| {
            OverrideResult::handled(FetchPeerSet { peers: Vec::new(), weak: true })
        });
    }
    graphs.push(node_sim);

    match fault {
        AdversarialFault::None => {}
        AdversarialFault::StalledReader => run.provider.schedule_stalled_reader(bad_peer, fault_at),
        AdversarialFault::SilentResponder => run.provider.schedule_silent_responder(bad_peer, fault_at),
    }

    let requested_peers = Arc::new(Mutex::new(Vec::new()));
    let (dispatch, _layer_peers) = block_requested_dispatch(requested_peers.clone());
    let mut world = WorldLoop::new(run.provider.clone(), graphs).with_injector(0, shared0.expect("injector 0"));
    world.set_subscriber(node_idx, dispatch);

    // Do not read the chain store from this callback: the tip write that woke us may
    // still hold it, and a re-entrant lock deadlocks the world loop.
    let mut last_tip_at = None;
    let mut mailbox_high = 0usize;
    world.run_until_horizon_on_best_chain_tip(horizon, |world| {
        last_tip_at = Some(world.now_nanos());
        let (len, _) = manager_mailbox(world, node_idx);
        mailbox_high = mailbox_high.max(len);
    });

    let (mailbox_len, mailbox_size) = manager_mailbox(&world, node_idx);
    mailbox_high = mailbox_high.max(mailbox_len);
    let finished_at = world.now_nanos();
    let adopted = adopted_head(&world, node_idx, &head);
    let adopted_at = adopted.then_some(last_tip_at).flatten();
    let ancestor_height = headers[0].block_height().as_u64();
    let mut heights = vec![ancestor_height];
    if let Some(height) = chain_tip_height(&world, node_idx)
        && height != ancestor_height
    {
        heights.push(height);
    }
    let log = world.heap_log();
    let fault_at = log.iter().find_map(|entry| match entry.kind {
        HeapLogKind::StalledReader { .. } | HeapLogKind::SilentResponder { .. } => Some(entry.time_nanos),
        HeapLogKind::Accepted { .. }
        | HeapLogKind::ConnectAttempt { .. }
        | HeapLogKind::ConnectTimeout { .. }
        | HeapLogKind::SendAck { .. }
        | HeapLogKind::Deliver { .. }
        | HeapLogKind::Close { .. }
        | HeapLogKind::PeerDisconnect
        | HeapLogKind::Reveal { .. }
        | HeapLogKind::GraphWake { .. } => None,
    });
    let requested_peers = std::mem::take(&mut *requested_peers.lock().expect("requested peers"));
    eprintln!(
        "adversarial {label} seed={:#x} adopted={adopted} adopted_at={adopted_at:?} finished_at={finished_at} heights={heights:?} mailbox={mailbox_len}/{mailbox_size} high={mailbox_high} requested={} fault_at={fault_at:?} last_peers={:?}",
        run.seed,
        requested_peers.len(),
        requested_peers.last(),
    );
    world.stop();
    AdversarialOutcome {
        seed: run.seed,
        heights,
        mailbox_size,
        mailbox_len,
        mailbox_high,
        requested_peers,
        finished_at,
        fault_at,
        adopted_at,
        bad_peer,
        honest_peers,
        adopted,
        log,
    }
}

fn chain_tip_height(world: &WorldLoop, graph: usize) -> Option<u64> {
    let store = world.graph(graph).resources().get::<ResourceHeaderStore>().expect("node chain store");
    let tip = store.get_best_chain_tip();
    store.load_header(&tip.hash()).map(|header| header.block_height().as_u64())
}

fn adopted_head(world: &WorldLoop, graph: usize, head: &Header) -> bool {
    let store = world.graph(graph).resources().get::<ResourceHeaderStore>().expect("node chain store");
    store.get_best_chain_tip() == head.point()
}

fn manager_mailbox(world: &WorldLoop, graph: usize) -> (usize, usize) {
    let manager = StageRef::<ManagerMessage>::named_for_tests("manager-1");
    let running = world.graph(graph);
    assert!(running.contains_stage(manager.name()), "node graph is missing manager-1");
    (running.mailbox_len(&manager), running.mailbox_size())
}

fn block_requested_dispatch(peers: Arc<Mutex<Vec<String>>>) -> (tracing::Dispatch, Arc<Mutex<Vec<String>>>) {
    let layer = RequestedPeers(peers.clone());
    let dispatch = tracing::Dispatch::new(registry().with(layer));
    (dispatch, peers)
}

struct RequestedPeers(Arc<Mutex<Vec<String>>>);

struct PeerField<'a>(&'a mut Option<String>);

impl Visit for PeerField<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "peers" && self.0.is_none() {
            *self.0 = Some(format!("{value:?}"));
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "peers" {
            *self.0 = Some(value.to_string());
        }
    }
}

impl<S: tracing::Subscriber> Layer<S> for RequestedPeers {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        if event.metadata().name() != "block.requested" {
            return;
        }
        let mut peers = None;
        event.record(&mut PeerField(&mut peers));
        if let Some(peers) = peers {
            self.0.lock().expect("requested peers").push(peers);
        }
    }
}

/// Bulk mailbox for the stalled-reader replay. An honest broadcast of this fragment
/// still fits; a peer that stops reading fills it when every later batch is offered there.
const STALL_MAILBOX: usize = 4;
/// Several block-fetch batches (`MAX_MISSING_BLOCKS_PER_BATCH` is 25).
const STALL_FRAGMENT: usize = 150;
/// After connects (about 3–8ms) and before this fragment's bodies are adopted (~150ms).
const STALL_FAULT_AT_NANOS: u64 = 10_000_000;
/// Honest adoption of [`STALL_FRAGMENT`] is about 150ms. The bound sits above that.
const STALL_ADOPTION_BOUND_NANOS: u64 = 500_000_000;
/// Long enough to record a late adoption. The 60s block-fetch agency timeout stays beyond it.
const STALL_HORIZON_NANOS: u64 = 2_000_000_000;

fn stall_replay(label: &str, fault: AdversarialFault) -> AdversarialOutcome {
    run_adversarial_with(AdversarialSetup {
        label,
        fault,
        mailbox: STALL_MAILBOX,
        fragment: STALL_FRAGMENT,
        fault_at: STALL_FAULT_AT_NANOS,
        horizon: STALL_HORIZON_NANOS,
        seed: None,
        broadcast: true,
    })
}

fn assert_head_before_bound(outcome: &AdversarialOutcome, label: &str) {
    assert_eq!(outcome.mailbox_size, STALL_MAILBOX, "{label} mailbox");
    assert!(
        outcome.adopted,
        "{label} must adopt the head from the other peers; seed={:#x} heights={:?} mailbox={}/{} high={} finished_at={}",
        outcome.seed,
        outcome.heights,
        outcome.mailbox_len,
        outcome.mailbox_size,
        outcome.mailbox_high,
        outcome.finished_at
    );
    assert!(
        outcome.adopted_at.is_some_and(|at| at < STALL_ADOPTION_BOUND_NANOS),
        "{label} must adopt before {}ms; seed={:#x} adopted_at={:?} finished_at={} high={}/{}",
        STALL_ADOPTION_BOUND_NANOS / 1_000_000,
        outcome.seed,
        outcome.adopted_at,
        outcome.finished_at,
        outcome.mailbox_high,
        outcome.mailbox_size
    );
    assert!(
        outcome.mailbox_high < outcome.mailbox_size,
        "{label} manager mailbox must stay below capacity; seed={:#x} high={}/{}",
        outcome.seed,
        outcome.mailbox_high,
        outcome.mailbox_size
    );
    assert!(outcome.heights.len() >= 2, "{label} chain advanced {:?}", outcome.heights);
    assert!(!outcome.requested_peers.is_empty(), "{label} must emit block.requested; seed={:#x}", outcome.seed);
    // Each connection logs its own peers. The last event can name only the lowest address,
    // which is healthy in the honest control, so the check is over the whole run.
    let asked_other = outcome
        .requested_peers
        .iter()
        .any(|peers| outcome.honest_peers.iter().any(|honest| peers.contains(&honest.to_string())));
    assert!(
        asked_other,
        "{label} must request blocks from a peer other than {}; seed={:#x} events={}",
        outcome.bad_peer,
        outcome.seed,
        outcome.requested_peers.len()
    );
}

/// Three peers, production mailbox, no fault. The small mailbox still syncs the fragment.
#[test]
fn test_world_small_mailbox_honest_peers_sync() {
    let outcome = run_adversarial("honest", AdversarialFault::None);
    assert_eq!(outcome.mailbox_size, ADMISSION_MAILBOX);
    assert!(outcome.fault_at.is_none(), "no fault was scheduled");
    assert!(outcome.adopted, "honest peers must sync at mailbox 10; seed={:#x}", outcome.seed);
    assert!(outcome.adopted_at.is_some(), "honest adoption time; seed={:#x}", outcome.seed);
    assert!(outcome.heights.len() >= 2, "chain advanced {:?}", outcome.heights);
    assert!(outcome.mailbox_len < outcome.mailbox_size, "manager mailbox {}", outcome.mailbox_len);
    assert!(!outcome.requested_peers.is_empty(), "block.requested; seed={:#x}", outcome.seed);
}

/// Every batch is offered to every peer, one of whom stops reading after the handshake.
/// The head is adopted from the others before [`STALL_ADOPTION_BOUND_NANOS`], and the
/// manager mailbox stays below [`STALL_MAILBOX`].
///
/// That bound fails when the manager's per-connection fetch send and the connection's
/// forward to the block-fetch handler both block. Either blocking send on its own still
/// adopts inside the bound: the other `try_send` skips a full mailbox and the fetch
/// continues on a peer that is still reading.
#[test]
fn test_world_stalled_reader_fetch_continues() {
    let outcome = stall_replay("stalled-reader", AdversarialFault::StalledReader);
    assert_head_before_bound(&outcome, "stalled reader");
    assert!(
        outcome.fault_at.is_some_and(|at| outcome.adopted_at.is_some_and(|adopted| adopted > at)),
        "adoption must follow the stall; seed={:#x} adopted_at={:?} fault_at={:?}",
        outcome.seed,
        outcome.adopted_at,
        outcome.fault_at
    );
    assert!(
        outcome
            .log
            .iter()
            .any(|entry| matches!(entry.kind, HeapLogKind::StalledReader { peer } if peer == outcome.bad_peer)),
        "stalled-reader hop; seed={:#x}",
        outcome.seed
    );
}

/// Same broadcast and mailbox as the stalled-reader replay, with every peer still reading.
#[test]
fn test_world_broadcast_honest_peers_sync() {
    let outcome = stall_replay("broadcast-honest", AdversarialFault::None);
    assert!(outcome.fault_at.is_none(), "no fault was scheduled");
    assert_head_before_bound(&outcome, "broadcast honest");
}

/// A silent peer still lets the others finish a short fragment. Writes are acked, so this
/// does not fill the manager mailbox and does not guard the fan-out.
#[test]
fn test_world_silent_responder_fetch_continues() {
    let outcome = run_adversarial("silent-responder", AdversarialFault::SilentResponder);
    assert_eq!(outcome.mailbox_size, ADMISSION_MAILBOX);
    assert!(outcome.fault_at.is_some(), "silent-responder hop must pop; seed={:#x}", outcome.seed);
    assert!(
        outcome.adopted_at.is_some_and(|at| at > outcome.fault_at.unwrap_or(0)),
        "silent responder must still be fetching when the fault pops; seed={:#x} adopted_at={:?} fault_at={:?}",
        outcome.seed,
        outcome.adopted_at,
        outcome.fault_at
    );
    assert!(
        outcome.adopted,
        "silent responder must still sync; seed={:#x} heights={:?}",
        outcome.seed, outcome.heights
    );
    assert!(outcome.heights.len() >= 2, "chain advanced {:?}", outcome.heights);
    assert!(
        outcome.mailbox_len < outcome.mailbox_size,
        "manager mailbox {}/{}",
        outcome.mailbox_len,
        outcome.mailbox_size
    );
    assert!(!outcome.requested_peers.is_empty(), "block.requested; seed={:#x}", outcome.seed);
    assert!(
        outcome
            .log
            .iter()
            .any(|entry| matches!(entry.kind, HeapLogKind::SilentResponder { peer } if peer == outcome.bad_peer)),
        "silent-responder hop; seed={:#x}",
        outcome.seed
    );
}

/// A peer that never reads stalls this node's writes. The local side does not close that
/// connection for a missing `SendAck`, so the teardown assertion stays ignored.
#[test]
#[ignore = "a connection whose peer never reads is not closed by the local side yet"]
fn test_stalled_reader_tears_down_only_that_connection() {
    let outcome = run_adversarial("stalled-teardown", AdversarialFault::StalledReader);
    let bad_responders: Vec<_> = outcome
        .log
        .iter()
        .filter_map(|entry| match entry.kind {
            HeapLogKind::Accepted { listener, responder_conn, .. } if listener == outcome.bad_peer => {
                Some(responder_conn)
            }
            HeapLogKind::Accepted { .. }
            | HeapLogKind::ConnectAttempt { .. }
            | HeapLogKind::ConnectTimeout { .. }
            | HeapLogKind::SendAck { .. }
            | HeapLogKind::Deliver { .. }
            | HeapLogKind::Close { .. }
            | HeapLogKind::PeerDisconnect
            | HeapLogKind::StalledReader { .. }
            | HeapLogKind::SilentResponder { .. }
            | HeapLogKind::Reveal { .. }
            | HeapLogKind::GraphWake { .. } => None,
        })
        .collect::<Vec<_>>();
    assert!(!bad_responders.is_empty(), "the stalled injector must have accepted; seed={:#x}", outcome.seed);
    assert!(
        outcome
            .log
            .iter()
            .any(|entry| matches!(entry.kind, HeapLogKind::Close { conn } if bad_responders.contains(&conn))),
        "the stalled connection must be torn down; seed={:#x}",
        outcome.seed
    );
    let honest_responders: Vec<_> = outcome
        .log
        .iter()
        .filter_map(|entry| match entry.kind {
            HeapLogKind::Accepted { listener, responder_conn, .. } if outcome.honest_peers.contains(&listener) => {
                Some(responder_conn)
            }
            HeapLogKind::Accepted { .. }
            | HeapLogKind::ConnectAttempt { .. }
            | HeapLogKind::ConnectTimeout { .. }
            | HeapLogKind::SendAck { .. }
            | HeapLogKind::Deliver { .. }
            | HeapLogKind::Close { .. }
            | HeapLogKind::PeerDisconnect
            | HeapLogKind::StalledReader { .. }
            | HeapLogKind::SilentResponder { .. }
            | HeapLogKind::Reveal { .. }
            | HeapLogKind::GraphWake { .. } => None,
        })
        .collect::<Vec<_>>();
    assert!(
        outcome.log.iter().all(|entry| match entry.kind {
            HeapLogKind::Close { conn } => !honest_responders.contains(&conn),
            HeapLogKind::Accepted { .. }
            | HeapLogKind::ConnectAttempt { .. }
            | HeapLogKind::ConnectTimeout { .. }
            | HeapLogKind::SendAck { .. }
            | HeapLogKind::Deliver { .. }
            | HeapLogKind::PeerDisconnect
            | HeapLogKind::StalledReader { .. }
            | HeapLogKind::SilentResponder { .. }
            | HeapLogKind::Reveal { .. }
            | HeapLogKind::GraphWake { .. } => true,
        }),
        "honest peers must stay up; seed={:#x}",
        outcome.seed
    );
}
