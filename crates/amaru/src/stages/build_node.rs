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

use crate::stages::build_stage_graph::build_stage_graph;
use crate::stages::config::{Config, StoreType};
use crate::stages::ledger::Ledger;
use amaru_consensus::effects::{ResourceBlockValidation, ResourceHeaderValidation, ResourceMeter};
use amaru_consensus::errors::ConsensusError;
use amaru_consensus::headers_tree::HeadersTreeState;
use amaru_consensus::stages::pull::SyncTracker;
use amaru_consensus::stages::select_chain::SelectChain;
use amaru_consensus::stages::validate_header::ValidateHeader;
use amaru_kernel::{
    BlockHeader, ConsensusParameters, EraHistory, GlobalParameters, HeaderHash, ORIGIN_HASH, Peer,
    Tip, Transaction,
};
use amaru_mempool::InMemoryMempool;
use amaru_metrics::METRICS_METER_NAME;
use amaru_network::connection::TokioConnections;
use amaru_ouroboros::{ChainStore, ConnectionsResource, HasStakeDistribution, ResourceMempool};
use amaru_protocols::manager::{Manager, ManagerConfig, ManagerMessage};
use amaru_protocols::store_effects::{ResourceHeaderStore, ResourceParameters};
use amaru_stores::rocksdb::consensus::RocksDBStore;
use anyhow::{Context, anyhow};
use opentelemetry::metrics::MeterProvider;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use pure_stage::tokio::{TokioBuilder, TokioRunning};
use pure_stage::{StageGraph, StageRef};
use std::sync::Arc;
use tokio::runtime::Handle;
use tracing::info;

/// Build a node given the provided configuration and run it.
pub fn build_and_run_node(
    config: Config,
    meter_provider: Option<SdkMeterProvider>,
) -> anyhow::Result<TokioRunning> {
    let mut stage_builder = TokioBuilder::default();
    build_node(
        &config,
        config.network.into(),
        meter_provider,
        &mut stage_builder,
    )?;

    Ok(stage_builder.run(Handle::current().clone()))
}

pub fn build_node(
    config: &Config,
    global_parameters: &GlobalParameters,
    meter_provider: Option<SdkMeterProvider>,
    stage_builder: &mut impl StageGraph,
) -> anyhow::Result<StageRef<ManagerMessage>> {
    let era_history: &EraHistory = config.network.into();

    // Make the ledger and get its tip
    let ledger = Ledger::new(config, era_history.clone(), global_parameters.clone())
        .context("Failed to create ledger. Have you bootstrapped your node?")?;

    let tip = ledger.get_tip();
    info!(
        tip.hash = %tip.hash(),
        tip.slot = u64::from(tip.slot_or_default()),
        "build_node"
    );

    // Make the chain store, either from the network resources if already set
    // or from the configuration.
    // This also makes sure that the chain store tip and anchors are is exactly aligned to the
    // ledger tip.
    let chain_store = initialize_chain_store(config, &tip.hash())?;
    let tip = chain_store.load_tip(&tip.hash()).unwrap_or(Tip::origin());

    // Make resources
    let peers = config.upstream_peers.iter().map(|p| Peer::new(p)).collect();
    let chain_selector = make_chain_selector(
        chain_store.clone(),
        &peers,
        global_parameters.consensus_security_param,
    )?;
    let validate_header = make_validate_header(
        global_parameters,
        era_history,
        chain_store.clone(),
        ledger.get_stake_distribution()?,
    );
    let manager = Manager::new(
        config.network_magic,
        ManagerConfig::default(),
        Arc::new(era_history.clone()),
    );

    // Register resources
    register_resources(
        stage_builder,
        chain_store,
        global_parameters,
        ledger,
        validate_header,
        meter_provider,
    );

    // Build the stage graph and return a reference to the manager stage
    let manager_stage = build_stage_graph(
        chain_selector,
        SyncTracker::new(&peers),
        manager,
        tip,
        stage_builder,
    );

    // Connect to upstream peers
    for peer in &config.upstream_peers {
        let Ok(_) =
            stage_builder.preload(&manager_stage, [ManagerMessage::AddPeer(Peer::new(peer))])
        else {
            tracing::warn!("supplied more peers than can be initially connected");
            break;
        };
    }

    // Open a port to listen for downstream peers
    stage_builder
        .preload(
            &manager_stage,
            [ManagerMessage::Listen(config.listen_address()?)],
        )
        .map_err(|e| anyhow!(format!("{e:?}")))?;
    Ok(manager_stage)
}

/// Register the resources required by the external effects invoked by the stages in the stage graph.
fn register_resources(
    stage_graph: &mut impl StageGraph,
    chain_store: Arc<dyn ChainStore<BlockHeader>>,
    global_parameters: &GlobalParameters,
    ledger: Ledger,
    validate_header: ValidateHeader,
    meter_provider: Option<SdkMeterProvider>,
) {
    stage_graph
        .resources()
        .put::<ResourceHeaderStore>(chain_store.clone());
    stage_graph
        .resources()
        .put::<ResourceParameters>(global_parameters.clone());
    stage_graph
        .resources()
        .put::<ResourceBlockValidation>(ledger.get_block_validation());
    stage_graph
        .resources()
        .put::<ResourceHeaderValidation>(Arc::new(validate_header));
    stage_graph
        .resources()
        .put::<ConnectionsResource>(Arc::new(TokioConnections::new(65535)));
    stage_graph
        .resources()
        .put::<ResourceMempool<Transaction>>(Arc::new(InMemoryMempool::default()));

    if let Some(provider) = meter_provider {
        let meter = provider.meter(METRICS_METER_NAME);
        stage_graph
            .resources()
            .put::<ResourceMeter>(Arc::new(meter));
    };
}

/// This function migrates the database if necessary and
/// sets the tip and anchor of the chain store to the ledger tip.
fn initialize_chain_store(
    config: &Config,
    tip: &HeaderHash,
) -> anyhow::Result<Arc<dyn ChainStore<BlockHeader>>> {
    let chain_store: Arc<dyn ChainStore<BlockHeader>> = match config.chain_store {
        StoreType::InMem(ref chain_store) => chain_store.clone(),
        StoreType::RocksDb(ref rocks_db_config) if config.migrate_chain_db => {
            Arc::new(RocksDBStore::open_and_migrate(rocks_db_config)?)
        }
        StoreType::RocksDb(ref rocks_db_config) => Arc::new(RocksDBStore::open(rocks_db_config)?),
    };

    if *tip != ORIGIN_HASH && chain_store.load_header(tip).is_none() {
        anyhow::bail!(
            "Tip {} not found in chain database '{}'",
            tip,
            config.chain_store
        )
    };

    chain_store.set_anchor_hash(tip)?;

    // Only reset best_chain_hash if it doesn't point to a valid header.
    // This allows tests to pre-populate the chain store with a chain
    // while still setting the anchor to the ledger tip.
    let current_best = chain_store.get_best_chain_hash();
    if chain_store.load_header(&current_best).is_none() {
        chain_store.set_best_chain_hash(tip)?;
    }

    Ok(chain_store)
}

fn make_chain_selector(
    chain_store: Arc<dyn ChainStore<BlockHeader>>,
    peers: &Vec<Peer>,
    consensus_security_parameter: usize,
) -> Result<SelectChain, ConsensusError> {
    let mut tree_state = HeadersTreeState::new(consensus_security_parameter);

    let anchor = chain_store.get_anchor_hash();
    for peer in peers {
        tree_state.initialize_peer(chain_store.clone(), peer, &anchor)?;
    }

    Ok(SelectChain::new(tree_state))
}

fn make_validate_header(
    global_parameters: &GlobalParameters,
    era_history: &EraHistory,
    chain_store: Arc<dyn ChainStore<BlockHeader>>,
    stake_distribution: Arc<dyn HasStakeDistribution>,
) -> ValidateHeader {
    let consensus_parameters = Arc::new(ConsensusParameters::new(
        global_parameters.clone(),
        era_history,
        Default::default(),
    ));

    ValidateHeader::new(consensus_parameters, chain_store, stake_distribution)
}
