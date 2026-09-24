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

use std::{collections::BTreeSet, path::PathBuf, sync::Arc, time::Duration};

use amaru_consensus::{
    block_validator::{BlockValidator, LedgerThreadJoinError, LedgerThreadStop},
    effects::{
        ResourceBlockValidation, ResourceConsensusParameters, ResourceEraHistory, ResourceHasStakePools, ResourceMeter,
        ResourcePoolSummaries, ResourceTxValidation, find_best_candidate,
    },
    performance::{Performance, ResourcePerformance},
    stages::track_peers::TrackPeersMsg,
};
use amaru_kernel::{ConsensusParameters, EraHistory, GlobalParameters, HeaderHash, PeerCandidate, Point, Transaction};
use amaru_ledger::{
    startup::{StartupHook, with_startup_hook},
    state::State,
    store::{OpenErrorKind, StoreError as LedgerStoreError},
};
use amaru_mempool::{InMemoryMempool, MempoolConfig};
use amaru_metrics::Meter;
use amaru_network::{connection::TokioConnections, resolve::init_resolver};
use amaru_observability::warn;
use amaru_ouroboros::{
    BaseReadChainStore, ChainStore, ConnectionsResource, MempoolMsg, PoolSummaries, ResourceMempool,
    StoreError as ChainStoreError,
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
use thiserror::Error;
use tokio::runtime::Handle;

use crate::{
    ClearValidity, realign_chain_store_to,
    stages::{
        build_stage_graph::{NodeStages, build_stage_graph},
        config::{Config, LedgerConfig, StoreType},
    },
};

const LEDGER_THREAD_STOP_TIMEOUT: Duration = Duration::from_secs(10);

/// A startup failure that an embedding host can handle without inspecting error text.
///
/// Listener binding happens after construction; this result does not establish network readiness.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum NodeStartError {
    #[error("invalid configuration: {reason}")]
    InvalidConfiguration { reason: String },
    #[error("store at '{}' is already in use", path.display())]
    StoreInUse { path: PathBuf },
    #[error("incompatible store at '{}': {source}", path.display())]
    IncompatibleStore {
        path: PathBuf,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error(
        "the chain database is inconsistent with the ledger: its best chain, ending at \
         {best_chain}, does not contain the ledger tip {ledger_tip}. This happens when \
         a ledger snapshot is imported on top of a chain database built for another chain. \
         Remove the chain database so that it can be rebuilt from the ledger tip."
    )]
    StorePairMismatch { ledger_tip: Point, best_chain: HeaderHash },
    #[error("{source}")]
    Other {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl NodeStartError {
    fn other(source: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        Self::Other { source: source.into() }
    }
}

impl From<anyhow::Error> for NodeStartError {
    fn from(error: anyhow::Error) -> Self {
        error.downcast::<Self>().unwrap_or_else(Self::other)
    }
}

struct NodeLifecycle {
    ledger_thread: LedgerThreadStop,
    connections: Arc<TokioConnections>,
    performance: Box<dyn FnOnce() -> std::thread::Result<()> + Send + Sync>,
}

/// Outcome of a shutdown that released every owned resource.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
#[must_use = "inspect unexpected_exits even when cleanup completed successfully"]
pub struct ShutdownReport {
    /// Components that terminated unexpectedly before or during shutdown.
    pub unexpected_exits: Vec<ComponentFailure>,
}

impl ShutdownReport {
    /// Whether every component exited as expected.
    pub fn is_clean(&self) -> bool {
        self.unexpected_exits.is_empty()
    }
}

/// A component failure observed during a completed shutdown.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ComponentFailure {
    Stages(String),
    NetworkListeners(String),
    Ledger(String),
    Performance(String),
}

/// Shutdown stopped without proving that every owned resource was released.
#[derive(Debug, Clone, Eq, Error, PartialEq)]
pub enum ShutdownError {
    #[error("timed out after {timeout:?} waiting for the ledger thread; stores may still be active")]
    LedgerTimeout { timeout: Duration },
    #[error("shutdown join task failed: {reason}; resources may still be active")]
    JoinTask { reason: String },
}

/// Build a node given the provided configuration and run it on `runtime`.
///
/// The Tokio [`Handle`] must be passed explicitly; this never uses ambient
/// `Handle::current()`. Metrics come from [`Config::meter`], or a default empty
/// [`Meter`] when unset.
///
/// For the common embedding path prefer [`crate::NodeBuilder`].
pub fn build_and_run_node(config: Config, runtime: &Handle) -> Result<NodeRunning, NodeStartError> {
    init_resolver().map_err(NodeStartError::other)?;
    let meter = config.meter.clone().unwrap_or_else(|| Arc::new(Meter::default()));
    let trace_buffer = TraceBuffer::new_shared(config.trace_buffer_min_entries, config.trace_buffer_max_size);
    let mut stage_builder = TokioBuilder::default()
        .with_trace_buffer(trace_buffer)
        .with_global_epoch_offset(config.compute_global_clock_offset());

    let node_stages = build_node(&config, config.global_parameters(), meter, &mut stage_builder)?;
    let lifecycle = stage_builder.resources().take::<NodeLifecycle>()?;
    let mempool_sender = stage_builder.input(node_stages.mempool_stage());
    let tokio_running = stage_builder.run(runtime.clone());
    Ok(NodeRunning { tokio_running, mempool_sender, lifecycle })
}

/// Unique owner of a running node and its final shutdown result.
pub struct NodeRunning {
    tokio_running: TokioRunning,
    mempool_sender: Sender<MempoolMsg>,
    lifecycle: NodeLifecycle,
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

    /// Return a non-blocking abort callback that does not own the node's shutdown result.
    pub fn abort_callback(&self) -> impl Fn() + Send + Sync + 'static {
        self.tokio_running.abort_callback()
    }

    /// Stop and join every node-owned task, then close listeners and stores.
    ///
    /// `Ok` means cleanup completed; inspect the report for component failures.
    /// `Err` means at least one resource may still be active and should not be reopened.
    /// Cancelling this future does not stop worker joins already running on the blocking pool
    /// and does not establish that cleanup completed.
    pub async fn shutdown(self) -> Result<ShutdownReport, ShutdownError> {
        let Self { tokio_running, mempool_sender, lifecycle } = self;
        let NodeLifecycle { ledger_thread, connections, performance } = lifecycle;

        tokio_running.request_abort();
        drop(mempool_sender);
        let stages = tokio_running.join().await.err().map(|error| ComponentFailure::Stages(error.to_string()));
        let listeners =
            connections.shutdown().await.err().map(|error| ComponentFailure::NetworkListeners(error.to_string()));
        drop(connections);
        let (ledger, performance) = tokio::task::spawn_blocking(move || {
            (ledger_thread.join_timeout(LEDGER_THREAD_STOP_TIMEOUT), performance())
        })
        .await
        .map_err(|error| ShutdownError::JoinTask { reason: error.to_string() })?;
        let performance = performance.err().map(|_| ComponentFailure::Performance("worker panicked".into()));
        let ledger = match ledger {
            Ok(()) => None,
            Err(LedgerThreadJoinError::Panicked(message)) => Some(ComponentFailure::Ledger(message)),
            Err(LedgerThreadJoinError::Timeout { timeout }) => return Err(ShutdownError::LedgerTimeout { timeout }),
        };

        Ok(ShutdownReport {
            unexpected_exits: [stages, listeners, ledger, performance].into_iter().flatten().collect(),
        })
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
    let listen_address = config
        .listen_address()
        .map_err(|error| NodeStartError::InvalidConfiguration { reason: format!("{error:#}") })?;
    // NOTE: Open the chain store first so incompatible DB versions fail before the slower ledger open.
    let chain_store = make_chain_store(config)?;

    // Make the ledger state and get its tip
    let mut state = make_state(&config.ledger_config, Some(with_startup_hook::<RocksDB>), chain_store.clone())?;
    state.set_observers(config.observers.clone());
    let ledger_tip = state.tip().into_owned();
    amaru_observability::info!(node::build::LEDGER_OPENED, tip = ledger_tip);

    let pool_summaries = state.pool_summaries();
    let max_epoch = pool_summaries.max_epoch();

    // Production restarts drop the volatile ledger, so the chain store can be ahead of the
    // persisted ledger tip. Rewind the best-chain pointer to that tip.
    if config.realign_chain_store {
        initialize_chain_store(chain_store.clone(), ledger_tip)?;
    }
    let ledger_tip = chain_store.load_point(&ledger_tip.hash()).ok_or(anyhow!("ledger tip header not found"))?;

    // The best hash for blocks that were possibly downloaded and validated before a restart,
    // i.e. before the volatile ledger was dropped.
    let recovery_best_hash = find_best_candidate(chain_store.as_ref())?;
    let block_validator = Arc::new(make_block_validator(&config.ledger_config, state, chain_store.clone())?);

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
    // Weak: the callback is stored on `block_validator`, which lives in these same
    // resources. A strong capture would leak every node (RocksDB FDs included).
    let resources = stage_builder.resources().downgrade();
    block_validator.set_on_stake_dist_updated(Arc::new(move |summaries| {
        let max_epoch = summaries.max_epoch();
        resources.put::<ResourcePoolSummaries>(Arc::new(summaries));
        let send = async {
            if track_peers_sender.send(TrackPeersMsg::StakeDistUpdated(max_epoch)).await.is_err() {
                amaru_observability::warn!(node::build::STAKE_DIST_NOTIFY_FAILED);
            }
        };
        // The callback runs on the ledger thread; its join also covers notification delivery.
        #[expect(clippy::expect_used)]
        let rt = tokio::runtime::Builder::new_current_thread().build().expect("cannot build current thread runtime");
        rt.block_on(send);
    }));

    // Open a port to listen for downstream peers
    stage_builder
        .preload(node_stages.manager_stage.clone(), [ManagerMessage::Listen(listen_address)])
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
    block_validator: Arc<BlockValidator>,
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
    let ledger_thread = block_validator.thread_stop();
    // NOTE: used in WorldLoop::stop() and impl Drop for World
    stage_graph.resources().put(ledger_thread.clone());
    stage_graph.resources().put::<ResourcePoolSummaries>(Arc::new(pool_summaries));
    let connections = Arc::new(TokioConnections::new(65535));
    stage_graph.resources().put::<ConnectionsResource>(connections.clone());
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
    let performance =
        Performance::with_peer_sources(static_peers, snapshot_candidates, Default::default(), config.peer_mix.clone());
    let join_performance = Box::new(performance.shutdown_callback());
    stage_graph.resources().put::<ResourcePerformance>(Arc::new(performance));

    stage_graph.resources().put(NodeLifecycle { ledger_thread, connections, performance: join_performance });
}

/// This function migrates the database if necessary
fn make_chain_store(config: &Config) -> anyhow::Result<Arc<dyn ChainStore>> {
    let chain_store: Arc<dyn ChainStore> = match config.chain_store {
        StoreType::InMem(ref chain_store) => chain_store.clone(),
        StoreType::RocksDb(ref rocks_db_config) => {
            let open = if config.migrate_chain_db { RocksDBStore::open_and_migrate } else { RocksDBStore::open };
            Arc::new(open(rocks_db_config).map_err(|error| match error {
                ChainStoreError::Locked { path } => NodeStartError::StoreInUse { path },
                error @ ChainStoreError::IncompatibleChainStoreVersions { .. } => {
                    NodeStartError::IncompatibleStore { path: rocks_db_config.dir.clone(), source: Box::new(error) }
                }
                error @ (ChainStoreError::WriteError { .. }
                | ChainStoreError::ReadError { .. }
                | ChainStoreError::OpenError { .. }) => NodeStartError::other(error),
            })?)
        }
    };

    Ok(chain_store)
}

pub fn make_block_validator(
    config: &LedgerConfig,
    state: State<RocksDB, RocksDBHistoricalStores>,
    chain_store: Arc<dyn ChainStore>,
) -> anyhow::Result<BlockValidator> {
    Ok(BlockValidator::new(
        state,
        ArenaPool::new(config.ledger_vm_alloc_arena_count, config.ledger_vm_alloc_arena_size),
        chain_store,
    )?)
}

pub fn make_state(
    config: &LedgerConfig,
    on_startup: Option<StartupHook<RocksDB>>,
    chain_store: Arc<dyn BaseReadChainStore>,
) -> anyhow::Result<State<RocksDB, RocksDBHistoricalStores>> {
    let store = RocksDB::new(&config.ledger_store).map_err(|error| match error {
        LedgerStoreError::Open(OpenErrorKind::Locked { file, .. }) => NodeStartError::StoreInUse { path: file },
        error @ (LedgerStoreError::Internal(_)
        | LedgerStoreError::Undecodable(_)
        | LedgerStoreError::Send
        | LedgerStoreError::Open(_)
        | LedgerStoreError::Missing(..)) => NodeStartError::other(error),
    })?;
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
    // Previously validated blocks have not been applied to this process's volatile ledger, so
    // their valid flags are stale. Invalid flags are cleared too: a false reject from an earlier
    // run must not hide that chain from `find_best_candidate` (which skips `valid=false` and does
    // not walk its children). Re-validation either repeats the error or, if the bug is gone, adopts
    // the chain. Runtime `FindBestCandidate` still skips blocks marked invalid in *this* run.
    realign_chain_store_to(chain_store.as_ref(), ledger_tip, ClearValidity::All)
}

#[cfg(test)]
mod tests {
    use std::net::{SocketAddr, TcpListener};

    use amaru_kernel::NetworkName;
    use amaru_stores::rocksdb::RocksDbConfig;
    use tokio::{net::TcpStream, time::timeout};

    use super::*;
    use crate::tests::configuration::NodeTestConfig;

    #[tokio::test(flavor = "multi_thread")]
    async fn shutdown_allows_starting_another_network() -> anyhow::Result<()> {
        let probe = TcpListener::bind("127.0.0.1:0")?;
        let listen_address = probe.local_addr()?;
        drop(probe);

        let stores = tempfile::tempdir()?;
        let mut configs = Vec::new();
        for network in [NetworkName::Preprod, NetworkName::Preview] {
            let test_config = NodeTestConfig::default()
                .with_network_name(network)
                .with_no_upstream_peers()
                .with_listen_address(&listen_address.to_string());
            let seeded = test_config.make_node_configuration()?;
            configs.push(
                test_config
                    .with_store_dirs(stores.path().join(network.to_string()), seeded.ledger_config.ledger_store.dir),
            );
        }

        let mut retained = Vec::new();
        for index in [0, 1, 0] {
            let test_config = &configs[index];
            let network = test_config.network_name;
            let mut config = test_config.make_node_configuration()?;
            config.ledger_config.global_parameters = network.as_global_parameters().unwrap().clone();
            config.migrate_chain_db = true;
            let ledger_config = config.ledger_config.ledger_store.clone();
            let chain_config = RocksDbConfig::new(stores.path().join(network.to_string()));

            let running = build_and_run_node(config, &Handle::current())?;
            retained.push((running.abort_callback(), running.mempool_sender()));
            wait_for_listener(listen_address).await?;
            assert!(timeout(Duration::from_secs(15), running.shutdown()).await??.is_clean());

            drop(TcpListener::bind(listen_address)?);
            drop(RocksDB::new(&ledger_config)?);
            drop(RocksDBStore::open(&chain_config)?);
            for (abort, sender) in &retained {
                abort();
                assert!(sender.send(MempoolMsg::NewTip(Point::Origin)).await.is_err());
            }
        }

        Ok(())
    }

    async fn wait_for_listener(address: SocketAddr) -> anyhow::Result<()> {
        timeout(Duration::from_secs(1), async {
            loop {
                if TcpStream::connect(address).await.is_ok() {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await?;
        Ok(())
    }
}
