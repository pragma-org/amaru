// Copyright 2024 PRAGMA
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

use std::{collections::BTreeSet, sync::Arc};

use amaru_consensus::{
    block_validator::BlockValidator,
    effects::{
        ResourceBlockValidation, ResourceConsensusParameters, ResourceEraHistory, ResourceHasStakePools, ResourceMeter,
        ResourcePoolSummaries, ResourceTxValidation, find_best_candidate,
    },
    performance::{Performance, ResourcePerformance},
    stages::track_peers::TrackPeersMsg,
};
use amaru_kernel::{ConsensusParameters, EraHistory, GlobalParameters, PeerCandidate, Point, Transaction};
use amaru_ledger::{
    startup::{StartupHook, with_startup_hook},
    state::State,
};
use amaru_mempool::{InMemoryMempool, MempoolConfig};
use amaru_metrics::Meter;
use amaru_network::connection::TokioConnections;
use amaru_observability::warn;
use amaru_ouroboros::{
    BaseReadChainStore, ChainStore, ConnectionsResource, MempoolMsg, PoolSummaries, ResourceMempool,
};
use amaru_plutus::arena_pool::ArenaPool;
use amaru_protocols::{
    manager::ManagerMessage,
    store_effects::{ResourceHeaderStore, ResourceParameters},
};
use amaru_pure_stage::{
    BoxFuture, Sender, StageGraph, StageGraphRunning,
    tokio::{TokioBuilder, TokioRunning},
    trace_buffer::TraceBuffer,
};
use amaru_stores::rocksdb::{RocksDB, RocksDBHistoricalStores, consensus::RocksDBStore};
use anyhow::anyhow;
use parking_lot::Mutex;
use tokio::runtime::Handle;

use crate::{
    ClearValidity, realign_chain_store_to,
    stages::{
        build_stage_graph::{NodeStages, build_stage_graph},
        config::{Config, LedgerConfig, StoreType},
    },
};

/// Build a node given the provided configuration and run it on `runtime`.
///
/// The Tokio [`Handle`] must be passed explicitly; this never uses ambient
/// `Handle::current()`. Metrics come from [`Config::meter`], or a default empty
/// [`Meter`] when unset.
///
/// For the common embedding path prefer [`crate::NodeBuilder`].
pub fn build_and_run_node(config: Config, runtime: &Handle) -> anyhow::Result<NodeRunning> {
    let meter = config.meter.clone().unwrap_or_else(|| Arc::new(Meter::default()));
    let trace_buffer = TraceBuffer::new_shared(config.trace_buffer_min_entries, config.trace_buffer_max_size);
    let mut stage_builder = TokioBuilder::default()
        .with_trace_buffer(trace_buffer)
        .with_global_epoch_offset(config.compute_global_clock_offset());

    let node_stages = build_node(&config, config.global_parameters(), meter, &mut stage_builder)?;
    let mempool_sender = stage_builder.input(node_stages.mempool_stage());
    let tokio_running = stage_builder.run(runtime.clone());
    Ok(NodeRunning { tokio_running, mempool_sender })
}

/// Encapsulation of the running runtime + accesses to entry points to the processing graph.
///
/// It gives us access to be the TokioRunning runtime and to specific input / output points for
/// the processing graph (just one for now, the mempool, but we can add more as needed).
#[derive(Clone)]
pub struct NodeRunning {
    tokio_running: TokioRunning,
    mempool_sender: Sender<MempoolMsg>,
}

impl NodeRunning {
    pub fn mempool_sender(&self) -> Sender<MempoolMsg> {
        self.mempool_sender.clone()
    }

    pub fn trace_buffer(&self) -> &Arc<Mutex<TraceBuffer>> {
        self.tokio_running.trace_buffer()
    }

    pub fn termination(&self) -> BoxFuture<'static, ()> {
        self.tokio_running.termination()
    }

    /// Abort all stage tasks without consuming this handle (safe from any thread).
    pub fn request_abort(&self) {
        self.tokio_running.request_abort();
    }

    pub fn abort(self) {
        self.tokio_running.abort();
    }
}

/// Build a node, given configuration parameters and a StageGraph implementation (could be `TokioBuilder` or `SimulationBuilder`):
///
/// 1. Initialize the ledger and get its tip.
/// 2. Initialize the chain store and its tip (make it equal to the ledger tip, because it could be further along than the ledger tip after a node stop).
/// 3. Prepare resources for the stages graph.
/// 4. Build the stages graph.
/// 5. The stage graph preloads `peer_selection` to connect to configured upstream peers.
/// 6. Register a listener for downstream connections.
///
/// Return a refererence to the `Manager` stage to have the possibility to send internal messages for
/// testing.
///
pub fn build_node(
    config: &Config,
    global_parameters: &GlobalParameters,
    meter: Arc<Meter>,
    stage_builder: &mut impl StageGraph,
) -> anyhow::Result<NodeStages> {
    // NOTE: Open the chain store first so incompatible DB versions fail before the slower ledger open.
    let chain_store = make_chain_store(config)?;

    // Make the ledger state and get its tip
    let mut state = make_state(&config.ledger_config, Some(with_startup_hook::<RocksDB>), chain_store.clone())?;
    state.set_observers(config.observers.clone());
    let ledger_tip = state.tip().into_owned();
    amaru_observability::info!(node::build::LEDGER_OPENED, tip = ledger_tip);

    let pool_summaries = state.pool_summaries();
    let block_validator = Arc::new(make_block_validator(&config.ledger_config, state, chain_store.clone())?);
    let max_epoch = pool_summaries.max_epoch();

    // Make the chain store, either from the network resources if already set
    // or from the configuration.
    // This also makes sure that the chain store tip and anchors are exactly aligned to the
    // ledger tip.
    initialize_chain_store(chain_store.clone(), ledger_tip)?;
    let ledger_tip = chain_store.load_point(&ledger_tip.hash()).ok_or(anyhow!("ledger tip header not found"))?;

    // The best hash for blocks that were possibly downloaded and validated before a restart,
    // i.e. before the volatile ledger was dropped.
    let recovery_best_hash = find_best_candidate(chain_store.as_ref())?;

    // Make resources
    let era_history = &config.era_history();

    let consensus_parameters = Arc::new(ConsensusParameters::new(global_parameters.clone(), config.era_history()));

    // Register resources
    register_resources(
        stage_builder,
        chain_store,
        global_parameters,
        pool_summaries,
        block_validator.clone(),
        consensus_parameters,
        config.era_history().clone(),
        meter,
        config.mempool.clone(),
        config,
    );
    let resources = stage_builder.resources().clone();

    // Build the stage graph and return a reference to the stages that can be connected from outside this function
    let node_stages = build_stage_graph(
        config,
        era_history,
        global_parameters,
        ledger_tip,
        recovery_best_hash,
        max_epoch,
        stage_builder,
    );

    let track_peers_sender = node_stages.track_peers_stake_dist_sender();
    block_validator.set_on_stake_dist_updated(Arc::new(move |summaries| {
        let max_epoch = summaries.max_epoch();
        resources.put::<ResourcePoolSummaries>(Arc::new(summaries));
        let track_peers_sender = track_peers_sender.clone();
        let send = async move {
            if track_peers_sender.send(TrackPeersMsg::StakeDistUpdated(max_epoch)).await.is_err() {
                amaru_observability::warn!(node::build::STAKE_DIST_NOTIFY_FAILED);
            }
        };
        #[expect(clippy::expect_used)]
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(send);
        } else {
            let rt =
                tokio::runtime::Builder::new_current_thread().build().expect("cannot build current thread runtime");
            rt.block_on(send);
        }
    }));
    // TODO: The runtime spawn/block_on hack above is required by the current Tokio integration
    // and ledger being driven from the main thread. It will be cleaned up when the ledger state
    // is handled in its own non-Tokio thread.

    // Open a port to listen for downstream peers
    stage_builder
        .preload(node_stages.manager_stage.clone(), [ManagerMessage::Listen(config.listen_address()?)])
        .map_err(|e| anyhow!(format!("{e:?}")))?;

    Ok(node_stages)
}

/// Register the resources required by the external effects invoked by the stages in the stage graph.
/// It is possible to override those resources later on.
#[allow(clippy::too_many_arguments)]
fn register_resources(
    stage_graph: &mut impl StageGraph,
    chain_store: Arc<dyn ChainStore>,
    global_parameters: &GlobalParameters,
    pool_summaries: PoolSummaries,
    block_validator: Arc<BlockValidator<RocksDB, RocksDBHistoricalStores>>,
    consensus_parameters: Arc<ConsensusParameters>,
    era_history: EraHistory,
    meter: Arc<Meter>,
    mempool_config: MempoolConfig,
    config: &Config,
) {
    stage_graph.resources().put::<ResourceHeaderStore>(chain_store);
    stage_graph.resources().put::<ResourceParameters>(global_parameters.clone());

    stage_graph.resources().put::<ResourceBlockValidation>(block_validator.clone());
    stage_graph.resources().put::<ResourceHasStakePools>(block_validator.clone());
    stage_graph.resources().put::<ResourceTxValidation>(block_validator.clone());
    stage_graph.resources().put::<ResourcePoolSummaries>(Arc::new(pool_summaries));
    stage_graph.resources().put::<ConnectionsResource>(Arc::new(TokioConnections::new(65535)));
    stage_graph.resources().put::<ResourceMempool<Transaction>>(Arc::new(InMemoryMempool::new(mempool_config)));

    stage_graph.resources().put::<ResourceConsensusParameters>(consensus_parameters);
    stage_graph.resources().put::<ResourceEraHistory>(era_history);

    stage_graph.resources().put::<ResourceMeter>(meter);

    let mut static_peers = BTreeSet::new();
    for address in &config.upstream_peers {
        match address.parse::<PeerCandidate>() {
            Ok(candidate) => {
                static_peers.insert(candidate);
            }
            Err(reason) => {
                warn!(protocols::peer_selection::peer::ADDRESS_REJECTED, address, reason = reason.to_string());
            }
        }
    }
    let snapshot_candidates = config
        .peer_snapshot_peers
        .iter()
        .copied()
        .map(PeerCandidate::from)
        .chain(config.peer_snapshot_unresolved.iter().cloned())
        .collect();
    stage_graph.resources().put::<ResourcePerformance>(Arc::new(Performance::with_peer_sources(
        static_peers,
        snapshot_candidates,
        Default::default(),
        config.peer_mix.clone(),
    )));
}

/// This function migrates the database if necessary
fn make_chain_store(config: &Config) -> anyhow::Result<Arc<dyn ChainStore>> {
    let chain_store: Arc<dyn ChainStore> = match config.chain_store {
        StoreType::InMem(ref chain_store) => chain_store.clone(),
        StoreType::RocksDb(ref rocks_db_config) if config.migrate_chain_db => {
            Arc::new(RocksDBStore::open_and_migrate(rocks_db_config)?)
        }
        StoreType::RocksDb(ref rocks_db_config) => Arc::new(RocksDBStore::open(rocks_db_config)?),
    };

    Ok(chain_store)
}

pub fn make_block_validator(
    config: &LedgerConfig,
    state: State<RocksDB, RocksDBHistoricalStores>,
    chain_store: Arc<dyn ChainStore>,
) -> anyhow::Result<BlockValidator<RocksDB, RocksDBHistoricalStores>> {
    Ok(BlockValidator::new(
        state,
        ArenaPool::new(config.ledger_vm_alloc_arena_count, config.ledger_vm_alloc_arena_size),
        chain_store,
    ))
}

pub fn make_state(
    config: &LedgerConfig,
    on_startup: Option<StartupHook<RocksDB>>,
    chain_store: Arc<dyn BaseReadChainStore>,
) -> anyhow::Result<State<RocksDB, RocksDBHistoricalStores>> {
    let store = RocksDB::new(&config.ledger_store)?;
    store.set_chain_store(chain_store);
    let snapshots = RocksDBHistoricalStores::new(&config.ledger_store, u64::from(config.max_extra_ledger_snapshots));
    Ok(State::new(
        store,
        snapshots,
        config.network,
        config.era_history().clone(),
        config.global_parameters.clone(),
        config.emit_initial_stake_distribution_progress_ticks,
        on_startup,
    )?)
}

fn initialize_chain_store(chain_store: Arc<dyn ChainStore>, ledger_tip: Point) -> anyhow::Result<()> {
    // Consider that previously validated blocks haven't been validated now, since the volatile
    // ledger is going to be reconstructed on a restart. Invalid flags are kept.
    realign_chain_store_to(chain_store.as_ref(), ledger_tip, ClearValidity::ValidOnly)
}
