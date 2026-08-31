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

//! Serve-only chain injector for world tests.
//!
//! This graph is not [`super::build_world_node`]. It speaks mux (handshake, keepalive,
//! ChainSync responder, BlockFetch responder), serves headers and blocks from a store
//! prefix, and does not run consensus or forge.
//!
//! The injector scans the source store at construction. WorldLoop holds the reveal
//! cursor and decides when each block becomes visible. The injector does not pick
//! the advertised tip on its own.

use std::{collections::BTreeSet, net::SocketAddr, sync::Arc, time::Duration};

use amaru_consensus::stages::select_chain::cmp_tip;
use amaru_kernel::{Header, HeaderHash, IsHeader, NetworkMagic, PREPROD_ERA_HISTORY, Peer, Point, Transaction};
use amaru_mempool::InMemoryMempool;
use amaru_metrics::Meter;
use amaru_ouroboros::{
    BaseReadChainStore, ConnectionsResource, ResourceMempool, WriteChainStore,
    in_memory_chain_store::InMemoryChainStore,
};
use amaru_protocols::{
    chainsync::{ChainSyncInitiatorMsg, InitiatorMessage, InitiatorResult},
    manager::{Manager, ManagerConfig, ManagerMessage},
    metrics_effects::ResourceMeter,
    store_effects::ResourceHeaderStore,
};
use amaru_pure_stage::{
    Effects, StageGraph, StageRef,
    simulation::{Fifo, SimulationBuilder, SimulationRunning},
    trace_buffer::TraceBuffer,
};
use parking_lot::Mutex;
use tokio::runtime::Handle;

/// Shared injector handle owned by [`super::WorldLoop`].
///
/// Inventory is scanned at construction. WorldLoop holds the reveal cursor and decides
/// when each block becomes visible. Do not hang this off [`SimulationRunning`].
pub struct InjectorShared {
    inner: Mutex<InjectorInner>,
}

struct InjectorInner {
    inventory: Vec<Point>,
    revealed: usize,
    manager: StageRef<ManagerMessage>,
    source: Arc<dyn BaseReadChainStore>,
    serving: Arc<InMemoryChainStore>,
}

impl InjectorShared {
    fn new(
        manager: StageRef<ManagerMessage>,
        source: Arc<dyn BaseReadChainStore>,
        serving: Arc<InMemoryChainStore>,
        inventory: Vec<Point>,
    ) -> Arc<Self> {
        Arc::new(Self { inner: Mutex::new(InjectorInner { inventory, revealed: 0, manager, source, serving }) })
    }

    pub(super) fn inventory_len(&self) -> usize {
        self.inner.lock().inventory.len()
    }

    pub(super) fn manager(&self) -> StageRef<ManagerMessage> {
        self.inner.lock().manager.clone()
    }

    /// Copy the source prefix through `hash` into the serving store and return that tip.
    pub(super) fn reveal_through(&self, hash: HeaderHash) -> anyhow::Result<Point> {
        let mut inner = self.inner.lock();
        let through = inner
            .inventory
            .iter()
            .position(|point| point.hash() == hash)
            .ok_or_else(|| anyhow::anyhow!("hash {hash} is not in the injector inventory"))?;
        while inner.revealed <= through {
            copy_revealed_block(inner.source.as_ref(), inner.serving.as_ref(), inner.inventory[inner.revealed])?;
            inner.revealed += 1;
        }
        Ok(inner.inventory[through])
    }
}

fn copy_revealed_block(
    source: &dyn BaseReadChainStore,
    serving: &InMemoryChainStore,
    point: Point,
) -> anyhow::Result<()> {
    let hash = point.hash();
    let header = source.load_header(&hash).ok_or_else(|| anyhow::anyhow!("missing header {hash}"))?;
    serving.store_header(&header)?;
    if let Some(body) = source.load_block(&hash)? {
        serving.store_block(&hash, &body)?;
    }
    if serving.get_anchor_point() == Point::Origin {
        serving.set_anchor_point(&header.point())?;
    }
    serving.roll_forward_chain(&header.point())?;
    Ok(())
}

/// Walk the disseminable fragment already stored in `store`.
///
/// Uses `next_best_chain` when that still names the fragment (InMemory / live tip).
/// After `realign_chain_store_to` the best-chain pointer is the snapshot, so this falls
/// back to walking children after that snapshot — the same tree recovery uses.
fn scan_inventory(store: &dyn BaseReadChainStore) -> Vec<Point> {
    let tip = store.get_best_chain_tip();
    let realigned =
        tip != Point::Origin && store.next_best_chain(&tip).is_none() && !store.get_children(&tip.hash()).is_empty();
    if realigned { children_fragment(store, tip.hash()) } else { walk_next_best(store, Point::Origin) }
}

fn walk_next_best(store: &dyn BaseReadChainStore, mut cursor: Point) -> Vec<Point> {
    let mut out = Vec::new();
    while let Some(next) = store.next_best_chain(&cursor) {
        let Some(header) = store.load_header(&next.hash()) else {
            break;
        };
        let has_body = store.has_block(&header.hash()).unwrap_or(false);
        out.push(header.point());
        if !has_body {
            break;
        }
        cursor = next;
    }
    out
}

fn children_fragment(store: &dyn BaseReadChainStore, after: HeaderHash) -> Vec<Point> {
    let mut best = None;
    let mut to_visit = store.get_children(&after);
    let mut seen = BTreeSet::new();
    while let Some(hash) = to_visit.pop() {
        if !seen.insert(hash) {
            continue;
        }
        let Some(header) = store.load_header(&hash) else {
            continue;
        };
        if store.has_block(&hash).unwrap_or(false)
            && best.as_ref().is_none_or(|current| cmp_tip(Some(&header), Some(current)).is_gt())
        {
            best = Some(header);
        }
        to_visit.extend(store.get_children(&hash));
    }
    let Some(head) = best else {
        return Vec::new();
    };
    let mut headers = vec![head];
    loop {
        let Some(parent) = headers.last().and_then(Header::parent) else {
            return Vec::new();
        };
        if parent == after {
            headers.reverse();
            return headers.into_iter().map(|header| header.point()).collect();
        }
        let Some(header) = store.load_header(&parent) else {
            return Vec::new();
        };
        headers.push(header);
    }
}

/// Build the serve-only injector graph.
///
/// Listens and accepts (`accept_interval = 0` so extra inbounds are not gated on the 100ms
/// Wait). Serves ChainSync/BlockFetch from an InMemory copy of the revealed prefix. The
/// source store is scanned at construction; after realign that source still advertises the snapshot.
pub fn build_injector(
    source: Arc<dyn BaseReadChainStore>,
    connections: ConnectionsResource,
    listen: SocketAddr,
    seed: u64,
    tokio_handle: &Handle,
) -> anyhow::Result<(SimulationRunning, Arc<InjectorShared>)> {
    let serving = Arc::new(InMemoryChainStore::new());
    let inventory = scan_inventory(source.as_ref());
    let mut stage_graph = SimulationBuilder::default()
        .with_seed(seed)
        .with_eval_strategy(Fifo)
        .with_trace_buffer(TraceBuffer::new_shared(10_000, 8_000_000));
    put_serve_resources(&mut stage_graph, connections, serving.clone());

    let manager = stage_graph.stage("manager", amaru_protocols::manager::stage);
    let manager = stage_graph.wire_up(
        manager,
        Manager::new(
            NetworkMagic::PREPROD,
            ManagerConfig::default().with_accept_interval(Duration::ZERO).with_reconnect_delay(Duration::ZERO),
            Arc::new(PREPROD_ERA_HISTORY.clone()),
            StageRef::blackhole(),
            StageRef::blackhole(),
            StageRef::blackhole(),
        ),
    );
    let manager_ref = manager.without_state();
    let shared = InjectorShared::new(manager_ref.clone(), source, serving, inventory);

    stage_graph
        .preload(&manager_ref, [ManagerMessage::Listen(listen)])
        .map_err(|_| anyhow::anyhow!("failed to preload injector Listen"))?;

    Ok((stage_graph.run(tokio_handle), shared))
}

fn put_serve_resources(
    stage_graph: &mut SimulationBuilder,
    connections: ConnectionsResource,
    store: Arc<InMemoryChainStore>,
) {
    stage_graph.resources().put::<ConnectionsResource>(connections);
    stage_graph.resources().put::<ResourceHeaderStore>(store);
    stage_graph.resources().put::<ResourceMempool<Transaction>>(Arc::new(InMemoryMempool::default()));
    stage_graph.resources().put::<ResourceMeter>(Arc::new(Meter::default()));
}

/// Thin ChainSync client used to observe injector reveals. Not [`super::build_world_node`].
pub fn build_injector_peer(
    connections: ConnectionsResource,
    injector: SocketAddr,
    seed: u64,
    tokio_handle: &Handle,
) -> anyhow::Result<SimulationRunning> {
    let store = Arc::new(InMemoryChainStore::new());
    let mut stage_graph = SimulationBuilder::default()
        .with_seed(seed)
        .with_eval_strategy(Fifo)
        .with_trace_buffer(TraceBuffer::new_shared(10_000, 8_000_000));
    put_serve_resources(&mut stage_graph, connections, store);

    let manager = stage_graph.stage("manager", amaru_protocols::manager::stage);
    let pipeline = stage_graph.stage("pipeline", peer_pipeline);
    let pipeline = stage_graph.wire_up(pipeline, ());
    let manager = stage_graph.wire_up(
        manager,
        Manager::new(
            NetworkMagic::PREPROD,
            ManagerConfig::default().with_accept_interval(Duration::ZERO).with_reconnect_delay(Duration::ZERO),
            Arc::new(PREPROD_ERA_HISTORY.clone()),
            pipeline.without_state(),
            StageRef::blackhole(),
            StageRef::blackhole(),
        ),
    );
    let manager_ref = manager.without_state();
    stage_graph
        .preload(&manager_ref, [ManagerMessage::AddPeer(Peer::new(&injector.to_string()))])
        .map_err(|_| anyhow::anyhow!("failed to preload injector peer AddPeer"))?;
    Ok(stage_graph.run(tokio_handle))
}

async fn peer_pipeline(_state: (), msg: ChainSyncInitiatorMsg, eff: Effects<ChainSyncInitiatorMsg>) {
    match msg.msg {
        InitiatorResult::Initialize | InitiatorResult::Terminated => {}
        InitiatorResult::IntersectFound(_, _)
        | InitiatorResult::IntersectNotFound(_)
        | InitiatorResult::RollForward(_, _)
        | InitiatorResult::RollBackward(_, _) => {
            eff.send(&msg.handler, InitiatorMessage::RequestNext).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use amaru_kernel::{
        BlockHeight, NetworkPoint, PREPROD_ERA_HISTORY, Slot, any_headers_chain_with_root,
        cardano::network_block::make_encoded_block, utils::tests::run_strategy,
    };
    use amaru_ouroboros::WriteChainStore;

    use super::*;

    #[test]
    fn test_scan_inventory_empty_store_is_empty() {
        let store = InMemoryChainStore::new();
        assert!(scan_inventory(&store).is_empty());
    }

    #[test]
    fn test_scan_inventory_lists_linear_fragment_in_chain_order() {
        let (store, headers) = primed_linear_store(3);
        let inventory = scan_inventory(&store);
        assert_eq!(inventory, headers.iter().map(Header::point).collect::<Vec<_>>());
    }

    fn primed_linear_store(n: usize) -> (InMemoryChainStore, Vec<Header>) {
        let conway_start_slot = Slot::from(68_774_400);
        let root = NetworkPoint::Specific(conway_start_slot, amaru_kernel::Hash::new([0u8; 32]));
        let headers = run_strategy(any_headers_chain_with_root(n, root.with_height(BlockHeight::from(0))));
        let store = InMemoryChainStore::new();
        store.set_anchor_point(&headers[0].point()).unwrap();
        for header in &headers {
            store.store_header(header).unwrap();
            store.store_block(&header.hash(), &make_encoded_block(header, &PREPROD_ERA_HISTORY)).unwrap();
            store.roll_forward_chain(&header.point()).unwrap();
        }
        (store, headers)
    }
}
