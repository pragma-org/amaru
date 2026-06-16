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

use amaru_consensus::effects::{
    ResourceBlockValidation, ResourceHasPeersData, ResourceHeaderValidation, ResourceMeter, ResourceTxValidation,
    find_best_candidate,
};
use amaru_kernel::{GlobalParameters, Transaction};
use amaru_mempool::{InMemoryMempool, MempoolConfig};
use amaru_metrics::METRICS_METER_NAME;
use amaru_network::connection::TokioConnections;
use amaru_ouroboros_traits::{ChainStore, ConnectionsResource, ResourceMempool};
use amaru_protocols::{
    manager::ManagerMessage,
    store_effects::{ResourceHeaderStore, ResourceParameters},
    tx_submission::MempoolMsg,
};
use amaru_pure_stage::{
    BoxFuture, Sender, StageGraph, StageGraphRunning,
    tokio::{TokioBuilder, TokioRunning},
    trace_buffer::TraceBuffer,
};
use amaru_stores::rocksdb::consensus::RocksDBStore;
use anyhow::{Context, anyhow};
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
    let mut stage_builder = TokioBuilder::default().with_trace_buffer(trace_buffer);

    let node_stages = build_node(&config, meter_provider, &mut stage_builder)?;
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
    meter_provider: Option<SdkMeterProvider>,
    stage_builder: &mut impl StageGraph,
) -> anyhow::Result<NodeStages> {
    // Create the chain store
    let chain_store = create_chain_store(config)?;
    // Make the ledger
    let ledger = Ledger::new(&config.ledger_config, chain_store.clone())
        .context("Failed to create ledger. Have you bootstrapped your node?")?;

    let ledger_tip = ledger.initialize_chain_store()?;
    let best_hash = find_best_candidate(chain_store.as_ref())?;

    // Register resources
    let global_parameters = config.global_parameters();
    register_resources(stage_builder, global_parameters, chain_store, ledger, meter_provider, config.mempool.clone());

    // Build the stage graph and return a reference to the stages that can be connected from outside this function
    let node_stages = build_stage_graph(config, ledger_tip, best_hash, stage_builder);

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
    global_parameters: &GlobalParameters,
    chain_store: Arc<dyn ChainStore>,
    ledger: Ledger,
    meter_provider: Option<SdkMeterProvider>,
    mempool_config: MempoolConfig,
) {
    stage_graph.resources().put::<ResourceHeaderStore>(chain_store);
    stage_graph.resources().put::<ResourceParameters>(global_parameters.clone());
    stage_graph.resources().put::<ResourceBlockValidation>(ledger.get_block_validation());
    stage_graph.resources().put::<ResourceHeaderValidation>(ledger.get_header_validation());
    stage_graph.resources().put::<ResourceTxValidation>(ledger.get_tx_validation());
    stage_graph.resources().put::<ResourceHasPeersData>(ledger.get_peers_data());
    stage_graph.resources().put::<ConnectionsResource>(Arc::new(TokioConnections::new(65535)));
    stage_graph.resources().put::<ResourceMempool<Transaction>>(Arc::new(InMemoryMempool::new(mempool_config)));

    if let Some(provider) = meter_provider {
        let meter = provider.meter(METRICS_METER_NAME);
        stage_graph.resources().put::<ResourceMeter>(Arc::new(meter));
    };
}

/// This function migrates the database if necessary
fn create_chain_store(config: &Config) -> anyhow::Result<Arc<dyn ChainStore>> {
    let store = match config.chain_store {
        StoreType::InMem(ref chain_store) => chain_store.clone(),
        StoreType::RocksDb(ref rocks_db_config) if config.migrate_chain_db => {
            Arc::new(RocksDBStore::open_and_migrate(rocks_db_config)?)
        }
        StoreType::RocksDb(ref rocks_db_config) => Arc::new(RocksDBStore::open(rocks_db_config)?),
    };
    Ok(store)
}
