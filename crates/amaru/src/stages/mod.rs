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

use crate::stages::{
    build_stage_graph::build_stage_graph,
    consensus::forward_chain::{tcp_forward_chain_server::TcpForwardChainServer, to_pallas_tip},
};
use amaru_consensus::consensus::{
    effects::{
        ResourceBlockValidation, ResourceForwardEventListener, ResourceHeaderStore,
        ResourceHeaderValidation, ResourceMeter, ResourceParameters,
    },
    errors::ConsensusError,
    headers_tree::HeadersTreeState,
    stages::{
        pull::{self, SyncTracker},
        select_chain::SelectChain,
        validate_header::ValidateHeader,
    },
};
use amaru_kernel::{
    ArenaPool, BlockHeader, EraHistory, HeaderHash, IsHeader, ORIGIN_HASH, Point,
    network::NetworkName,
    peer::Peer,
    protocol_messages::{network_magic::NetworkMagic, tip::Tip},
    protocol_parameters::{ConsensusParameters, GlobalParameters},
};
use amaru_ledger::block_validator::BlockValidator;
use amaru_metrics::METRICS_METER_NAME;
use amaru_network::connection::TokioConnections;
use amaru_ouroboros_traits::{
    CanValidateBlocks, ChainStore, ConnectionResource, HasStakeDistribution,
    in_memory_consensus_store::InMemConsensusStore,
};
use amaru_protocols::manager;
use amaru_stores::{
    in_memory::MemoryStore,
    rocksdb::{RocksDB, RocksDBHistoricalStores, RocksDbConfig, consensus::RocksDBStore},
};
use anyhow::Context;
use opentelemetry::metrics::MeterProvider;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use pure_stage::{
    StageGraph,
    tokio::{TokioBuilder, TokioRunning},
};
use std::{
    fmt::{Debug, Display},
    path::PathBuf,
    sync::Arc,
};
use tokio::runtime::Handle;
use tracing::info;

pub mod build_stage_graph;
pub mod consensus;

/// Whether or not data is stored on disk or in memory.
#[derive(Clone)]
pub enum StoreType<S> {
    InMem(S),
    RocksDb(RocksDbConfig),
}

impl<S> Display for StoreType<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreType::InMem(..) => write!(f, "<mem>"),
            StoreType::RocksDb(config) => write!(f, "{}", config),
        }
    }
}

pub struct Config {
    pub ledger_store: StoreType<MemoryStore>,
    pub chain_store: StoreType<()>,
    pub upstream_peers: Vec<String>,
    pub network: NetworkName,
    pub network_magic: NetworkMagic,
    pub listen_address: String,
    pub max_downstream_peers: usize,
    pub max_extra_ledger_snapshots: MaxExtraLedgerSnapshots,
    pub migrate_chain_db: bool,

    // Number of allocation arenas to keep around for performing parallel evaluation of scripts in
    // the ledger.
    pub ledger_vm_alloc_arena_count: usize,

    // Initial size (in bytes) of each allocation arena to use for script evaluation in the ledger
    // virtual machine. Higher sizes means less re-allocations but more resident memory footprint
    // since the arena is leaking memory on purpose.
    pub ledger_vm_alloc_arena_size: usize,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            ledger_store: StoreType::RocksDb(RocksDbConfig::new(PathBuf::from("./ledger.db"))),
            chain_store: StoreType::RocksDb(RocksDbConfig::new(PathBuf::from("./chain.db"))),
            upstream_peers: vec![],
            network: NetworkName::Preprod,
            network_magic: NetworkMagic::PREPROD,
            listen_address: "0.0.0.0:3000".to_string(),
            max_downstream_peers: 10,
            max_extra_ledger_snapshots: MaxExtraLedgerSnapshots::default(),
            migrate_chain_db: false,
            ledger_vm_alloc_arena_count: 1,
            ledger_vm_alloc_arena_size: 1_024_000,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum MaxExtraLedgerSnapshots {
    All,
    UpTo(u64),
}

impl Default for MaxExtraLedgerSnapshots {
    fn default() -> Self {
        Self::UpTo(0)
    }
}

impl std::fmt::Display for MaxExtraLedgerSnapshots {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::All => f.write_str("all"),
            Self::UpTo(n) => write!(f, "{n}"),
        }
    }
}

impl std::str::FromStr for MaxExtraLedgerSnapshots {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "all" => Ok(Self::All),
            _ => match s.parse() {
                Ok(e) => Ok(Self::UpTo(e)),
                Err(e) => Err(format!(
                    "invalid max ledger snapshot, cannot parse value: {e}"
                )),
            },
        }
    }
}

impl From<MaxExtraLedgerSnapshots> for u64 {
    fn from(max_extra_ledger_snapshots: MaxExtraLedgerSnapshots) -> Self {
        match max_extra_ledger_snapshots {
            MaxExtraLedgerSnapshots::All => u64::MAX,
            MaxExtraLedgerSnapshots::UpTo(n) => n,
        }
    }
}

pub async fn build_and_run_network(
    config: Config,
    meter_provider: Option<SdkMeterProvider>,
) -> anyhow::Result<TokioRunning> {
    let era_history: &EraHistory = config.network.into();

    let global_parameters: &GlobalParameters = config.network.into();

    let ledger = make_ledger(&config, era_history.clone(), global_parameters.clone())
        .context("Failed to create ledger. Have you bootstrapped your node?")?;

    let tip = ledger.get_tip();

    info!(
        tip.hash = %tip.hash(),
        tip.slot = u64::from(tip.slot_or_default()),
        "starting"
    );

    let chain_store = make_chain_store(&config, &tip.hash())?;
    let our_tip = chain_store
        .load_header(&tip.hash())
        .map(|h| h.tip())
        .unwrap_or(Tip::origin());

    let peers = config.upstream_peers.iter().map(|p| Peer::new(p)).collect();
    let chain_selector = make_chain_selector(
        chain_store.clone(),
        &peers,
        global_parameters.consensus_security_param,
    )?;

    let consensus_parameters = Arc::new(ConsensusParameters::new(
        global_parameters.clone(),
        era_history,
        Default::default(),
    ));
    let validate_header = ValidateHeader::new(
        consensus_parameters,
        chain_store.clone(),
        ledger.get_stake_distribution(),
    );

    let sync_tracker = SyncTracker::new(&peers);

    let forward_event_listener = Arc::new(
        TcpForwardChainServer::new(
            chain_store.clone(),
            config.listen_address.clone(),
            config.network_magic.as_u64(),
            config.max_downstream_peers,
            to_pallas_tip(our_tip),
        )
        .await?,
    );

    let mut network = TokioBuilder::default();

    let manager = network.stage("manager", manager::stage);
    let receive_header_stage =
        build_stage_graph(chain_selector, our_tip, manager.sender(), &mut network);

    let pull_stage = network.stage("pull", pull::stage);
    let pull_stage = network.wire_up(pull_stage, (sync_tracker, receive_header_stage));

    let manager = network.wire_up(
        manager,
        manager::Manager::new(config.network_magic, pull_stage.without_state()),
    );
    for peer in &peers {
        let Ok(_) = network.preload(&manager, [manager::ManagerMessage::AddPeer(peer.clone())])
        else {
            tracing::warn!("supplied more peers than can be initially connected");
            break;
        };
    }

    // Create the stages for the new network protocols stack
    // let conn = ConnectionResource::new(65535);
    // let conn_id = create_upstream_connection(&peers, &conn).await?;
    // let connection = network.stage("connection", connection::stage);
    // let connection = network.wire_up(
    //     connection,
    //     connection::Connection::new(
    //         conn_id,
    //         Role::Initiator,
    //         NetworkMagic::new(config.network_magic as u64),
    //     ),
    // );
    // network
    //     .preload(connection, [ConnectionMessage::Initialize])
    //     .map_err(|e| anyhow!("{e}"))?;
    //
    // network.resources().put(conn);
    // network
    //     .resources()
    //     .put::<ResourceMempool<Tx>>(Arc::new(InMemoryMempool::default()));

    // Register resources
    network
        .resources()
        .put::<ResourceHeaderStore>(chain_store.clone());
    network
        .resources()
        .put::<ResourceParameters>(global_parameters.clone());
    network
        .resources()
        .put::<ResourceBlockValidation>(ledger.get_block_validation());
    network
        .resources()
        .put::<ResourceHeaderValidation>(Arc::new(validate_header));
    network
        .resources()
        .put::<ResourceForwardEventListener>(forward_event_listener);
    network
        .resources()
        .put::<ConnectionResource>(Arc::new(TokioConnections::new(65535)));

    if let Some(provider) = meter_provider {
        let meter = provider.meter(METRICS_METER_NAME);
        network.resources().put::<ResourceMeter>(Arc::new(meter));
    };

    Ok(network.run(Handle::current().clone()))
}

// / Temporary function to create a connection to an upstream peer.
// / This will be replaced by some proper peer management.
// pub async fn create_upstream_connection(
//     peers: &[Peer],
//     conn: &ConnectionResource,
// ) -> anyhow::Result<ConnectionId> {
//     if let Some(peer) = peers.first() {
//         timeout(Duration::from_secs(5), async {
//             match ToSocketAddrs::String(env::var("PEER").unwrap_or(peer.to_string()))
//                 .resolve()
//                 .await
//             {
//                 Ok(addr) => conn.connect(addr).await.map_err(anyhow::Error::from),
//                 Err(e) => Err(anyhow::anyhow!(
//                     "Failed to resolve address for upstream peer {}: {}",
//                     peer,
//                     e
//                 )),
//             }
//         })
//         .await?
//     } else {
//         Err(anyhow::anyhow!(
//             "No upstream peers configured to connect to"
//         ))
//     }
// }

#[expect(clippy::panic)]
fn make_chain_store(
    config: &Config,
    tip: &HeaderHash,
) -> anyhow::Result<Arc<dyn ChainStore<BlockHeader>>> {
    let chain_store: Arc<dyn ChainStore<BlockHeader>> = match config.chain_store {
        StoreType::InMem(()) => Arc::new(InMemConsensusStore::new()),
        StoreType::RocksDb(ref rocks_db_config) if config.migrate_chain_db => {
            Arc::new(RocksDBStore::open_and_migrate(rocks_db_config)?)
        }
        StoreType::RocksDb(ref rocks_db_config) => Arc::new(RocksDBStore::open(rocks_db_config)?),
    };

    if *tip != ORIGIN_HASH && chain_store.load_header(tip).is_none() {
        panic!(
            "Tip {} not found in chain database '{}'",
            tip, config.chain_store
        )
    };

    chain_store.set_anchor_hash(tip)?;
    chain_store.set_best_chain_hash(tip)?;
    Ok(chain_store)
}

enum LedgerStage {
    InMemLedgerStage(BlockValidator<MemoryStore, MemoryStore>),
    OnDiskLedgerStage(BlockValidator<RocksDB, RocksDBHistoricalStores>),
}

impl LedgerStage {
    fn get_tip(&self) -> Point {
        match self {
            LedgerStage::InMemLedgerStage(stage) => stage.get_tip(),
            LedgerStage::OnDiskLedgerStage(stage) => stage.get_tip(),
        }
    }

    #[expect(clippy::unwrap_used)]
    fn get_stake_distribution(&self) -> Arc<dyn HasStakeDistribution> {
        match self {
            LedgerStage::InMemLedgerStage(stage) => {
                let state = stage.state.lock().unwrap();
                Arc::new(state.view_stake_distribution())
            }
            LedgerStage::OnDiskLedgerStage(stage) => {
                let state = stage.state.lock().unwrap();
                Arc::new(state.view_stake_distribution())
            }
        }
    }

    fn get_block_validation(self) -> Arc<dyn CanValidateBlocks + Send + Sync> {
        match self {
            LedgerStage::InMemLedgerStage(stage) => Arc::new(stage),
            LedgerStage::OnDiskLedgerStage(stage) => Arc::new(stage),
        }
    }
}

fn make_ledger(
    config: &Config,
    era_history: EraHistory,
    global_parameters: GlobalParameters,
) -> anyhow::Result<LedgerStage> {
    let vm_eval_pool = ArenaPool::new(
        config.ledger_vm_alloc_arena_count,
        config.ledger_vm_alloc_arena_size,
    );

    match &config.ledger_store {
        StoreType::InMem(store) => {
            let ledger = BlockValidator::new(
                store.clone(),
                store.clone(),
                vm_eval_pool,
                config.network,
                era_history,
                global_parameters,
            )?;
            Ok(LedgerStage::InMemLedgerStage(ledger))
        }
        StoreType::RocksDb(rocks_db_config) => {
            let ledger = BlockValidator::new(
                RocksDB::new(rocks_db_config)?,
                RocksDBHistoricalStores::new(
                    rocks_db_config,
                    u64::from(config.max_extra_ledger_snapshots),
                ),
                vm_eval_pool,
                config.network,
                era_history,
                global_parameters,
            )?;
            Ok(LedgerStage::OnDiskLedgerStage(ledger))
        }
    }
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

pub trait AsTip {
    fn as_tip(&self) -> Tip;
}

impl<H: IsHeader> AsTip for H {
    fn as_tip(&self) -> Tip {
        Tip::new(self.point(), self.block_height())
    }
}

#[cfg(test)]
mod tests {
    use amaru_stores::rocksdb::RocksDbConfig;
    use std::path::PathBuf;

    use super::StoreType;

    #[test]
    fn test_store_path_display() {
        assert_eq!(format!("{}", StoreType::InMem(())), "<mem>");
        assert_eq!(
            format!(
                "{}",
                StoreType::<()>::RocksDb(RocksDbConfig::new(PathBuf::from("/path/to/store")))
            ),
            "RocksDbConfig { dir: /path/to/store }"
        );
        assert_eq!(
            format!(
                "{}",
                StoreType::<()>::RocksDb(RocksDbConfig::new(PathBuf::from("./relative/path")))
            ),
            "RocksDbConfig { dir: ./relative/path }"
        );
        assert_eq!(
            format!(
                "{}",
                StoreType::<()>::RocksDb(RocksDbConfig::new(PathBuf::from("")))
            ),
            "RocksDbConfig { dir:  }"
        );
    }
}
