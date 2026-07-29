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
        ResourceBlockValidation, ResourceConsensusParameters, ResourceEraHistory, ResourceHasStakePools, ResourceMeter,
        ResourcePoolSummaries, ResourceTxValidation, find_best_candidate,
    },
    stages::track_peers::TrackPeersMsg,
};
use amaru_kernel::{ConsensusParameters, EraHistory, GlobalParameters, ORIGIN_HASH, Point, Transaction};
use amaru_ledger::state::State;
use amaru_mempool::{InMemoryMempool, MempoolConfig};
use amaru_network::connection::TokioConnections;
use amaru_observability::{debug, info, info_record};
use amaru_ouroboros::{ChainStore, ConnectionsResource, MempoolMsg, PoolSummaries, ResourceMempool};
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
use anyhow::{anyhow, bail};
use opentelemetry::metrics::Meter;
use parking_lot::Mutex;
use tokio::runtime::Handle;

use crate::stages::{
    build_stage_graph::{NodeStages, build_stage_graph},
    config::{Config, LedgerConfig, StoreType},
};

/// Build a node given the provided configuration and run it using Tokio.
pub fn build_and_run_node(config: Config, meter: Option<Meter>) -> anyhow::Result<NodeRunning> {
    let trace_buffer = TraceBuffer::new_shared(config.trace_buffer_min_entries, config.trace_buffer_max_size);
    let mut stage_builder = TokioBuilder::default()
        .with_trace_buffer(trace_buffer)
        .with_global_epoch_offset(config.compute_global_clock_offset());

    let node_stages = build_node(&config, config.global_parameters(), meter, &mut stage_builder)?;
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
    meter: Option<Meter>,
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
    let pool_summaries = state.pool_summaries();
    let block_validator = Arc::new(make_block_validator(&config.ledger_config, state, chain_store.clone())?);
    let max_epoch = pool_summaries.max_epoch();

    // Make the chain store, either from the network resources if already set
    // or from the configuration.
    // This also makes sure that the chain store tip and anchors are exactly aligned to the
    // ledger tip.
    initialize_chain_store(chain_store.clone(), ledger_tip)?;
    let ledger_tip = chain_store.load_tip(&ledger_tip.hash()).ok_or(anyhow!("ledger tip header not found"))?;

    // The best hash for blocks that were possibly downloaded and validated before a restart,
    // i.e. before the volatile ledger was dropped.
    let recovery_best_hash = find_best_candidate(chain_store.as_ref())?;

    // Make resources
    let era_history = &config.era_history();

    let consensus_parameters =
        Arc::new(ConsensusParameters::new(global_parameters.clone(), config.era_history(), Default::default()));

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
    meter: Option<Meter>,
    mempool_config: MempoolConfig,
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

    if let Some(meter) = meter {
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
    info!(consensus::chain_db::INITIALIZE, ledger_tip = %ledger_tip);

    let best_chain_hash = chain_store.get_best_chain_hash();
    let has_best_chain = best_chain_hash != ORIGIN_HASH;

    // Every fork branches at or after the immutable tip, which cannot be rolled back, so the
    // ledger tip is always on the recorded best chain. When it is not, the two databases describe
    // different chains and truncating the best chain would silently discard headers. This is
    // checked before any mutation so that a rejected chain database is left untouched.
    if has_best_chain && chain_store.load_from_best_chain(&ledger_tip).is_none() {
        bail!(
            "the chain database is inconsistent with the ledger: its best chain, ending at \
             {best_chain_hash}, does not contain the ledger tip {ledger_tip}. This happens when \
             a ledger snapshot is imported on top of a chain database built for another chain. \
             Remove the chain database so that it can be rebuilt from the ledger tip."
        );
    }

    chain_store.set_anchor_hash(&ledger_tip.hash())?;
    chain_store.set_block_valid(&ledger_tip.hash(), true)?;
    if has_best_chain {
        chain_store.switch_to_fork(&ledger_tip, &[])?;
    } else {
        chain_store.roll_forward_chain(&ledger_tip)?;
    }

    info_record!(consensus::chain_db::INITIALIZE, best_chain_hash = best_chain_hash);
    clear_valid_descendants_after_ledger_tip(chain_store.as_ref(), ledger_tip)?;
    Ok(())
}

/// Consider that previously validated blocks haven't been validated now, since the volatile ledger
/// is going to be reconstructed on a restart.
fn clear_valid_descendants_after_ledger_tip(chain_store: &dyn ChainStore, ledger_tip: Point) -> anyhow::Result<()> {
    let mut to_visit = chain_store.get_children(&ledger_tip.hash());
    let mut count = 0;

    while let Some(hash) = to_visit.pop() {
        let Some((_header, validity)) = chain_store.load_header_with_validity(&hash) else {
            continue;
        };

        if validity == Some(true) {
            count += 1;
            chain_store.remove_block_valid(&hash)?;
        }

        to_visit.extend(chain_store.get_children(&hash));
    }
    debug!(consensus::chain_db::CLEAR_VALID_DESCENDANTS, count = count);
    Ok(())
}

#[cfg(test)]
mod tests {
    use amaru_kernel::{BlockHeader, IsHeader, make_header};
    use amaru_ouroboros::{BaseReadChainStore, WriteChainStore, in_memory_chain_store::InMemoryChainStore};

    use super::*;

    #[test]
    fn realign_the_chain_store_on_the_ledger_tip() {
        // h0 -- h1 -- h2 -- h3   the chain the consensus had validated
        //         \
        //          h2a          a fork that was validated and rejected
        let h0 = header(1, 1, None);
        let h1 = header(2, 2, Some(&h0));
        let h2 = header(3, 3, Some(&h1));
        let h3 = header(4, 4, Some(&h2));
        let h2a = header(3, 30, Some(&h1));

        let chain_store = Arc::new(InMemoryChainStore::new());
        for header in [&h0, &h1, &h2, &h3, &h2a] {
            chain_store.store_header(header).unwrap();
            chain_store.set_block_valid(&header.hash(), true).unwrap();
        }
        chain_store.set_block_valid(&h2a.hash(), false).unwrap();
        for header in [&h0, &h1, &h2, &h3] {
            chain_store.roll_forward_chain(&header.point()).unwrap();
        }
        chain_store.set_anchor_hash(&h0.hash()).unwrap();

        // the ledger restarts from h1, behind the blocks the consensus had validated
        initialize_chain_store(chain_store.clone(), h1.point()).unwrap();

        assert_eq!(chain_store.get_anchor_hash(), h1.hash(), "the anchor must move to the ledger tip");
        assert_eq!(chain_store.get_best_chain_hash(), h1.hash(), "the best chain must end at the ledger tip");
        assert!(chain_store.load_from_best_chain(&h1.point()).is_some(), "the ledger tip stays on the best chain");
        assert!(chain_store.load_from_best_chain(&h2.point()).is_none(), "h2 must leave the best chain");
        assert!(chain_store.load_from_best_chain(&h3.point()).is_none(), "h3 must leave the best chain");

        assert_eq!(validity(chain_store.as_ref(), &h0), Some(true), "blocks before the ledger tip stay validated");
        assert_eq!(validity(chain_store.as_ref(), &h1), Some(true), "the ledger tip stays validated");
        assert_eq!(validity(chain_store.as_ref(), &h2), None, "h2 must be applied to the ledger again");
        assert_eq!(validity(chain_store.as_ref(), &h3), None, "h3 must be applied to the ledger again");
        assert_eq!(validity(chain_store.as_ref(), &h2a), Some(false), "an invalid block was never applied");
    }

    #[test]
    fn start_the_best_chain_on_a_store_that_has_none() {
        let h0 = header(1, 1, None);

        let chain_store = Arc::new(InMemoryChainStore::new());
        chain_store.store_header(&h0).unwrap();

        initialize_chain_store(chain_store.clone(), h0.point()).unwrap();

        assert_eq!(chain_store.get_anchor_hash(), h0.hash());
        assert_eq!(chain_store.get_best_chain_hash(), h0.hash());
        assert!(chain_store.load_from_best_chain(&h0.point()).is_some(), "the ledger tip starts the best chain");
        assert_eq!(validity(chain_store.as_ref(), &h0), Some(true));
    }

    #[test]
    fn reject_a_chain_store_that_does_not_contain_the_ledger_tip() {
        // h0 -- h1     the chain the consensus had built
        //   \
        //    h1a       the branch the ledger was bootstrapped on
        let h0 = header(1, 1, None);
        let h1 = header(2, 2, Some(&h0));
        let h1a = header(2, 20, Some(&h0));

        let chain_store = Arc::new(InMemoryChainStore::new());
        for header in [&h0, &h1, &h1a] {
            chain_store.store_header(header).unwrap();
        }
        for header in [&h0, &h1] {
            chain_store.roll_forward_chain(&header.point()).unwrap();
        }
        chain_store.set_anchor_hash(&h0.hash()).unwrap();

        let error = initialize_chain_store(chain_store.clone(), h1a.point()).unwrap_err().to_string();

        assert!(error.contains("inconsistent with the ledger"), "unexpected error: {error}");
        assert_eq!(chain_store.get_best_chain_hash(), h1.hash(), "the best chain must be left untouched");
        assert_eq!(chain_store.get_anchor_hash(), h0.hash(), "the anchor must be left untouched");
        for header in [&h0, &h1, &h1a] {
            assert_eq!(validity(chain_store.as_ref(), header), None, "no block validity must be recorded");
        }
    }

    // HELPERS

    fn header(block_height: u64, slot: u64, parent: Option<&BlockHeader>) -> BlockHeader {
        BlockHeader::from(make_header(block_height, slot, parent.map(BlockHeader::hash)))
    }

    fn validity(chain_store: &dyn ChainStore, header: &BlockHeader) -> Option<bool> {
        chain_store.load_header_with_validity(&header.hash()).and_then(|(_, validity)| validity)
    }
}
