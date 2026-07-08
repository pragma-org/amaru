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
    effects::{
        ResourceBlockValidation, ResourceConsensusParameters, ResourceEraHistory, ResourceHasStakePools, ResourceMeter,
        ResourcePoolSummaries, ResourceTxValidation, find_best_candidate,
    },
    stages::track_peers::TrackPeersMsg,
};
use amaru_kernel::{ConsensusParameters, EraHistory, GlobalParameters, ORIGIN_HASH, Point, Transaction};
use amaru_mempool::{InMemoryMempool, MempoolConfig};
use amaru_metrics::METRICS_METER_NAME;
use amaru_network::connection::TokioConnections;
use amaru_ouroboros::{ChainStore, ConnectionsResource, MempoolMsg, PoolSummaries, ResourceMempool};
use amaru_protocols::{
    manager::ManagerMessage,
    store_effects::{ResourceHeaderStore, ResourceParameters},
};
use amaru_pure_stage::{
    BoxFuture, Sender, StageGraph, StageGraphRunning,
    tokio::{TokioBuilder, TokioRunning},
    trace_buffer::TraceBuffer,
};
use amaru_stores::rocksdb::consensus::RocksDBStore;
use anyhow::anyhow;
use opentelemetry::metrics::MeterProvider;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use parking_lot::Mutex;
use tokio::runtime::Handle;

use crate::stages::{
    build_stage_graph::{NodeStages, build_stage_graph},
    config::{Config, StoreType},
    ledger::Ledger,
};

/// Build a node given the provided configuration and run it using Tokio.
pub fn build_and_run_node(config: Config, meter_provider: Option<SdkMeterProvider>) -> anyhow::Result<NodeRunning> {
    let trace_buffer = TraceBuffer::new_shared(config.trace_buffer_min_entries, config.trace_buffer_max_size);
    let mut stage_builder = TokioBuilder::default()
        .with_trace_buffer(trace_buffer)
        .with_global_epoch_offset(config.compute_global_clock_offset());

    let node_stages = build_node(&config, &config.global_parameters, meter_provider, &mut stage_builder)?;
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
    let era_history = &config.era_history;

    // Make the ledger and get its tip
    let ledger = Ledger::new(config, era_history.clone(), global_parameters.clone())?;

    let ledger_tip = ledger.get_tip();
    tracing::info!(
        tip.hash = %ledger_tip.hash(),
        tip.slot = u64::from(ledger_tip.slot_or_default()),
        "build_ledger"
    );

    // Make the chain store, either from the network resources if already set
    // or from the configuration.
    // This also makes sure that the chain store tip and anchors are exactly aligned to the
    // ledger tip.
    let chain_store = initialize_chain_store(config, ledger_tip)?;
    let ledger_tip = chain_store.load_tip(&ledger_tip.hash()).ok_or(anyhow!("ledger tip header not found"))?;
    let best_hash = find_best_candidate(chain_store.as_ref())?;

    let pool_summaries = ledger.get_pool_summaries()?;
    let initial_max_epoch = pool_summaries.max_epoch();

    // Build the stage graph first to obtain a Sender<TrackPeersMsg> via stage_graph.input().
    // This sender is required to notify track_peers from outside pure-stage (e.g. ledger's
    // on_stake_dist_updated hook).
    let node_stages = build_stage_graph(
        config,
        era_history,
        global_parameters,
        ledger_tip,
        best_hash,
        initial_max_epoch,
        stage_builder,
    );
    let track_peers_sender = node_stages.track_peers_stake_dist_sender();

    let resources = stage_builder.resources().clone();
    ledger.set_on_stake_dist_updated(Arc::new(move |summ| {
        let max_epoch = summ.max_epoch();
        resources.put::<ResourcePoolSummaries>(Arc::new(summ));
        let track_peers_sender = track_peers_sender.clone();
        let send = async move {
            if track_peers_sender.send(TrackPeersMsg::StakeDistUpdated(max_epoch)).await.is_err() {
                tracing::warn!("failed to send TrackPeersMsg::StakeDistUpdated");
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

    let consensus_parameters = Arc::new(ConsensusParameters::new(global_parameters.clone(), &era_history.clone()));

    // Register resources
    register_resources(
        stage_builder,
        chain_store,
        global_parameters.clone(),
        ledger,
        consensus_parameters,
        era_history.clone(),
        pool_summaries,
        meter_provider,
        config.mempool.clone(),
    );

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
    global_parameters: GlobalParameters,
    ledger: Ledger,
    consensus_parameters: Arc<ConsensusParameters>,
    era_history: EraHistory,
    pool_summaries: PoolSummaries,
    meter_provider: Option<SdkMeterProvider>,
    mempool_config: MempoolConfig,
) {
    stage_graph.resources().put::<ResourceHeaderStore>(chain_store);
    stage_graph.resources().put::<ResourceParameters>(global_parameters);
    stage_graph.resources().put::<ResourceBlockValidation>(ledger.get_block_validation());
    stage_graph.resources().put::<ResourceHasStakePools>(ledger.get_stake_pools());
    stage_graph.resources().put::<ResourceTxValidation>(ledger.get_tx_validation());
    stage_graph.resources().put::<ConnectionsResource>(Arc::new(TokioConnections::new(65535)));
    stage_graph.resources().put::<ResourceMempool<Transaction>>(Arc::new(InMemoryMempool::new(mempool_config)));

    stage_graph.resources().put::<ResourceConsensusParameters>(consensus_parameters);
    stage_graph.resources().put::<ResourceEraHistory>(era_history);
    stage_graph.resources().put::<ResourcePoolSummaries>(Arc::new(pool_summaries));

    if let Some(provider) = meter_provider {
        let meter = provider.meter(METRICS_METER_NAME);
        stage_graph.resources().put::<ResourceMeter>(Arc::new(meter));
    };
}

/// This function migrates the database if necessary
fn initialize_chain_store(config: &Config, ledger_tip: Point) -> anyhow::Result<Arc<dyn ChainStore>> {
    let chain_store: Arc<dyn ChainStore> = match config.chain_store {
        StoreType::InMem(ref chain_store) => chain_store.clone(),
        StoreType::RocksDb(ref rocks_db_config) if config.migrate_chain_db => {
            Arc::new(RocksDBStore::open_and_migrate(rocks_db_config)?)
        }
        StoreType::RocksDb(ref rocks_db_config) => Arc::new(RocksDBStore::open(rocks_db_config)?),
    };

    let anchor_hash = chain_store.get_anchor_hash();

    // This corresponds to a bootstrap, we need to correctly initialize the chain store
    if anchor_hash == ORIGIN_HASH {
        tracing::info!(anchor = %ledger_tip, "first initialization - setting anchor and best chain");
        chain_store.set_anchor_hash(&ledger_tip.hash())?;
        chain_store.set_block_valid(&ledger_tip.hash(), true)?;
        chain_store.roll_forward_chain(&ledger_tip)?;
    }

    Ok(chain_store)
}
