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

use std::sync::Arc;

use amaru_consensus::{
    block_validator::BlockValidator,
    effects::{
        ResourceBlockValidation, ResourceHasStakePools, ResourceHeaderValidation, ResourceMeter, ResourceTxValidation,
        find_best_candidate,
    },
    header_validator::HeaderValidator,
};
use amaru_kernel::{ConsensusParameters, EraHistory, GlobalParameters, ORIGIN_HASH, Point, Transaction};
use amaru_ledger::state::State;
use amaru_mempool::{InMemoryMempool, MempoolConfig};
use amaru_metrics::METRICS_METER_NAME;
use amaru_network::connection::TokioConnections;
use amaru_ouroboros::{
    ChainStore, ConnectionsResource, HasStakeDistribution, HasStakePools, MempoolMsg, ResourceMempool,
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
use opentelemetry::metrics::MeterProvider;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use parking_lot::Mutex;
use tokio::runtime::Handle;

use crate::stages::{
    build_stage_graph::{NodeStages, build_stage_graph},
    config::{Config, LedgerConfig, StoreType},
};

/// Build a node given the provided configuration and run it using Tokio.
pub fn build_and_run_node(config: Config, meter_provider: Option<SdkMeterProvider>) -> anyhow::Result<NodeRunning> {
    let trace_buffer = TraceBuffer::new_shared(config.trace_buffer_min_entries, config.trace_buffer_max_size);
    let mut stage_builder = TokioBuilder::default().with_trace_buffer(trace_buffer);

    let node_stages = build_node(&config, config.global_parameters(), meter_provider, &mut stage_builder)?;
    let mempool_sender = stage_builder.input(node_stages.mempool_stage());
    let tokio_running = stage_builder.run(Handle::current().clone());
    Ok(NodeRunning { tokio_running, mempool_sender })
}

/// Encapsulation of the running runtime + accesses to entry points to the processing graph.
///
/// It gives us access to be the TokioRunning runtime and to specific input / output points for
/// the processing graph (just one for now, the mempool, but we can add more as needed).
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
    meter_provider: Option<SdkMeterProvider>,
    stage_builder: &mut impl StageGraph,
) -> anyhow::Result<NodeStages> {
    // Make the ledger state and get its tip
    let state = make_state(&config.ledger_config)?;
    let ledger_tip = state.tip().into_owned();
    tracing::info!(
        tip.hash = %ledger_tip.hash(),
        tip.slot = u64::from(ledger_tip.slot_or_default()),
        "build_ledger"
    );

    let chain_store = make_chain_store(config)?;
    let stake_distribution_observer = Arc::new(state.view_stake_distribution());
    let stake_pools_observer = Arc::new(state.view_stake_pools());
    let block_validator = make_block_validator(&config.ledger_config, state, chain_store.clone())?;

    // Make the chain store, either from the network resources if already set
    // or from the configuration.
    // This also makes sure that the chain store tip and anchors are exactly aligned to the
    // ledger tip.
    initialize_chain_store(chain_store.clone(), ledger_tip)?;
    let ledger_tip = chain_store.load_tip(&ledger_tip.hash()).ok_or(anyhow!("ledger tip header not found"))?;
    let best_hash = find_best_candidate(chain_store.as_ref())?;

    // Make resources
    let era_history = &config.era_history();
    let validate_header =
        make_validate_header(global_parameters, era_history, chain_store.clone(), stake_distribution_observer);

    // Register resources
    register_resources(
        stage_builder,
        chain_store,
        global_parameters,
        block_validator,
        stake_pools_observer,
        validate_header,
        meter_provider,
        config.mempool.clone(),
    );

    // Build the stage graph and return a reference to the stages that can be connected from outside this function
    let node_stages = build_stage_graph(config, era_history, global_parameters, ledger_tip, best_hash, stage_builder);

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
    block_validator: BlockValidator<RocksDB, RocksDBHistoricalStores>,
    stake_pools_observer: Arc<dyn HasStakePools>,
    validate_header: HeaderValidator,
    meter_provider: Option<SdkMeterProvider>,
    mempool_config: MempoolConfig,
) {
    let block_validator = Arc::new(block_validator);
    stage_graph.resources().put::<ResourceHeaderStore>(chain_store);
    stage_graph.resources().put::<ResourceParameters>(global_parameters.clone());
    stage_graph.resources().put::<ResourceBlockValidation>(block_validator.clone());
    stage_graph.resources().put::<ResourceTxValidation>(block_validator);
    stage_graph.resources().put::<ResourceHasStakePools>(stake_pools_observer);
    stage_graph.resources().put::<ResourceHeaderValidation>(Arc::new(validate_header));
    stage_graph.resources().put::<ConnectionsResource>(Arc::new(TokioConnections::new(65535)));
    stage_graph.resources().put::<ResourceMempool<Transaction>>(Arc::new(InMemoryMempool::new(mempool_config)));

    if let Some(provider) = meter_provider {
        let meter = provider.meter(METRICS_METER_NAME);
        stage_graph.resources().put::<ResourceMeter>(Arc::new(meter));
    };
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

pub fn make_state(config: &LedgerConfig) -> anyhow::Result<State<RocksDB, RocksDBHistoricalStores>> {
    let store = RocksDB::new(&config.ledger_store)?;
    let snapshots = RocksDBHistoricalStores::new(&config.ledger_store, u64::from(config.max_extra_ledger_snapshots));
    Ok(State::new(store, snapshots, config.network, config.era_history().clone(), config.global_parameters.clone())?)
}

fn initialize_chain_store(chain_store: Arc<dyn ChainStore>, ledger_tip: Point) -> anyhow::Result<()> {
    let anchor_hash = chain_store.get_anchor_hash();

    // This corresponds to a bootstrap, we need to correctly initialize the chain store
    if anchor_hash == ORIGIN_HASH {
        tracing::info!(anchor = %ledger_tip, "first initialization - setting anchor and best chain");
        chain_store.set_anchor_hash(&ledger_tip.hash())?;
        chain_store.set_block_valid(&ledger_tip.hash(), true)?;
        chain_store.roll_forward_chain(&ledger_tip)?;
    }
    Ok(())
}

fn make_validate_header(
    global_parameters: &GlobalParameters,
    era_history: &EraHistory,
    chain_store: Arc<dyn ChainStore>,
    stake_distribution: Arc<dyn HasStakeDistribution>,
) -> HeaderValidator {
    let consensus_parameters =
        Arc::new(ConsensusParameters::new(global_parameters.clone(), era_history, Default::default()));

    HeaderValidator::new(consensus_parameters, chain_store, stake_distribution)
}
