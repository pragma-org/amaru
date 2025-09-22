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
    consensus::forward_chain::tcp_forward_chain_server::TcpForwardChainServer,
    metrics::MetricsStage,
    pure_stage_util::{PureStageSim, SendAdapter},
};
use amaru_consensus::consensus::{
    effects::{
        block_effects::{ResourceBlockFetcher, ResourceParameters},
        network_effects::ResourceForwardEventListener,
        store_effects::ResourceHeaderStore,
    },
    errors::ConsensusError,
    events::ChainSyncEvent,
    headers_tree::HeadersTree,
    stages::{
        fetch_block::ClientsBlockFetcher, select_chain::SelectChain,
        validate_block::ResourceBlockValidation,
    },
    tip::{AsHeaderTip, HeaderTip},
};
use amaru_kernel::{
    EraHistory, HEADER_HASH_SIZE, Hash, Header, ORIGIN_HASH, Point, network::NetworkName,
    peer::Peer, protocol_parameters::GlobalParameters,
};
use amaru_ledger::block_validator::BlockValidator;
use amaru_metrics::MetricsPort;
use amaru_network::block_fetch_client::PallasBlockFetchClient;
use amaru_ouroboros_traits::{
    CanFetchBlock, CanValidateBlocks, ChainStore, HasStakeDistribution, IsHeader,
    in_memory_consensus_store::InMemConsensusStore,
};
use amaru_stores::{
    in_memory::MemoryStore,
    rocksdb::{RocksDB, RocksDBHistoricalStores, consensus::RocksDBStore},
};
use gasket::{
    messaging::OutputPort,
    runtime::{self, Tether, spawn_stage},
};
use opentelemetry_sdk::metrics::SdkMeterProvider;
use pallas_network::{
    facades::PeerClient,
    miniprotocols::chainsync::{Client, HeaderContent, Tip},
};
use pure_stage::{StageGraph, tokio::TokioBuilder};
use std::{
    error::Error,
    fmt::{Debug, Display},
    path::PathBuf,
    sync::Arc,
};
use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;
use tracing::info;

pub mod build_stage_graph;
pub mod common;
pub mod consensus;
pub mod metrics;
pub mod pull;
mod pure_stage_util;

pub type BlockHash = Hash<32>;

/// Whether or not data is stored on disk or in memory.
#[derive(Clone)]
pub enum StorePath<S> {
    InMem(S),
    OnDisk(PathBuf),
}

impl<S> Display for StorePath<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorePath::InMem(..) => write!(f, "<mem>"),
            StorePath::OnDisk(path) => write!(f, "{}", path.display()),
        }
    }
}

pub struct Config {
    pub ledger_store: StorePath<MemoryStore>,
    pub chain_store: StorePath<()>,
    pub upstream_peers: Vec<String>,
    pub network: NetworkName,
    pub network_magic: u32,
    pub listen_address: String,
    pub max_downstream_peers: usize,
    pub max_extra_ledger_snapshots: MaxExtraLedgerSnapshots,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            ledger_store: StorePath::OnDisk(PathBuf::from("./ledger.db")),
            chain_store: StorePath::OnDisk(PathBuf::from("./chain.db")),
            upstream_peers: vec![],
            network: NetworkName::Preprod,
            network_magic: 1,
            listen_address: "0.0.0.0:3000".to_string(),
            max_downstream_peers: 10,
            max_extra_ledger_snapshots: MaxExtraLedgerSnapshots::default(),
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

pub async fn bootstrap(
    config: Config,
    clients: Vec<(String, PeerClient)>,
    exit: CancellationToken,
    metrics_provider: Option<SdkMeterProvider>,
) -> Result<Vec<Tether>, Box<dyn Error>> {
    let era_history: &EraHistory = config.network.into();

    let global_parameters: &GlobalParameters = config.network.into();

    let peers: Vec<Peer> = clients.iter().map(|c| Peer::new(&c.0)).collect();

    let ledger = make_ledger(
        &config,
        config.network,
        era_history.clone(),
        global_parameters.clone(),
    )
    .map_err(|e| -> Box<dyn Error> {
        format!(
            "Failed to create ledger. Have you bootstrapped your node? Error: {}",
            e
        )
        .into()
    })?;

    let (chain_syncs, block_fetchs): (
        Vec<(Peer, Client<HeaderContent>)>,
        Vec<(Peer, Arc<dyn CanFetchBlock>)>,
    ) = clients
        .into_iter()
        .map(|(peer_name, client)| {
            let PeerClient {
                chainsync,
                blockfetch,
                ..
            } = client;
            let peer = Peer::new(&peer_name);
            let block_fetch_client: Arc<dyn CanFetchBlock> =
                Arc::new(PallasBlockFetchClient::new(&peer, blockfetch));
            (
                (peer.clone(), chainsync),
                (peer.clone(), block_fetch_client),
            )
        })
        .collect();

    let tip = ledger.get_tip();

    info!(
        tip.hash = %tip.hash(),
        tip.slot = u64::from(tip.slot_or_default()),
        "starting"
    );

    let mut stages = chain_syncs
        .into_iter()
        .map(|session| pull::Stage::new(session.0, session.1, vec![tip.clone()]))
        .collect::<Vec<_>>();

    let chain_store = make_chain_store(&config, era_history, &tip.hash())?;
    let our_tip = chain_store
        .load_header(&tip.hash())
        .map(|h| h.as_header_tip())
        .unwrap_or(HeaderTip::new(Point::Origin, 0));

    let chain_selector = make_chain_selector(
        chain_store.clone(),
        &peers,
        global_parameters.consensus_security_param,
    )?;

    let forward_event_listener = Arc::new(
        TcpForwardChainServer::new(
            chain_store.clone(),
            config.listen_address.clone(),
            config.network_magic as u64,
            config.max_downstream_peers,
            our_tip.clone(),
        )
        .await?,
    );

    let mut metrics_stage = MetricsStage::new(metrics_provider);

    // start pure-stage parts, whose lifecycle is managed by a single gasket stage
    let mut network = TokioBuilder::default();

    let (to_metrics, from_stages) = gasket::messaging::tokio::mpsc_channel(50);

    let mut metrics_downstream: MetricsPort = Default::default();
    metrics_downstream.connect(to_metrics.clone());

    let graph_input = build_stage_graph(
        global_parameters,
        ledger.get_stake_distribution(),
        chain_selector,
        our_tip,
        &mut network,
    );
    let graph_input = network.input(&graph_input);

    network.resources().put::<ResourceHeaderStore>(chain_store);
    network
        .resources()
        .put::<ResourceParameters>(global_parameters.clone());
    network
        .resources()
        .put::<ResourceBlockFetcher>(Arc::new(ClientsBlockFetcher::new(block_fetchs)));
    network
        .resources()
        .put::<ResourceBlockValidation>(ledger.get_block_validation());
    network
        .resources()
        .put::<ResourceForwardEventListener>(forward_event_listener);

    let network = network.run(Handle::current().clone());
    let pure_stages = PureStageSim::new(network, exit);

    let outputs: Vec<&mut OutputPort<ChainSyncEvent>> = stages
        .iter_mut()
        .map(|p| &mut p.downstream)
        .collect::<Vec<_>>();

    for output in outputs {
        output.connect(SendAdapter(graph_input.clone()));
    }

    let (to_metrics, from_stages) = gasket::messaging::tokio::mpsc_channel(50);
    stages.iter_mut().for_each(|stage| {
        // These channels are meant to be cloned so they can be shared between threads
        stage.metrics_downstream.connect(to_metrics.clone());
    });

    metrics_stage.upstream.connect(from_stages);

    // No retry, crash on panics.
    let policy = runtime::Policy::default();

    let mut stages = stages
        .into_iter()
        .map(|p| spawn_stage(p, policy.clone()))
        .collect::<Vec<_>>();

    let pure_stages = spawn_stage(pure_stages, policy.clone());
    let metrics = spawn_stage(metrics_stage, policy.clone());

    stages.push(pure_stages);
    stages.push(metrics);
    Ok(stages)
}

#[expect(clippy::panic)]
fn make_chain_store(
    config: &Config,
    era_history: &EraHistory,
    tip: &Hash<HEADER_HASH_SIZE>,
) -> Result<Arc<dyn ChainStore<Header>>, Box<dyn Error>> {
    let chain_store: Arc<dyn ChainStore<Header>> = match config.chain_store {
        StorePath::InMem(()) => Arc::new(InMemConsensusStore::new()),
        StorePath::OnDisk(ref chain_dir) => Arc::new(RocksDBStore::new(chain_dir, era_history)?),
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
    network: NetworkName,
    era_history: EraHistory,
    global_parameters: GlobalParameters,
) -> anyhow::Result<LedgerStage> {
    match &config.ledger_store {
        StorePath::InMem(store) => {
            let ledger = BlockValidator::new(
                store.clone(),
                store.clone(),
                network,
                era_history,
                global_parameters,
            )?;
            Ok(LedgerStage::InMemLedgerStage(ledger))
        }
        StorePath::OnDisk(ledger_dir) => {
            let ledger = BlockValidator::new(
                RocksDB::new(ledger_dir)?,
                RocksDBHistoricalStores::new(
                    ledger_dir,
                    u64::from(config.max_extra_ledger_snapshots),
                ),
                network,
                era_history,
                global_parameters,
            )?;
            Ok(LedgerStage::OnDiskLedgerStage(ledger))
        }
    }
}

fn make_chain_selector(
    chain_store: Arc<dyn ChainStore<Header>>,
    peers: &Vec<Peer>,
    consensus_security_parameter: usize,
) -> Result<SelectChain, ConsensusError> {
    let mut tree = HeadersTree::new(chain_store.clone(), consensus_security_parameter);

    for peer in peers {
        tree.initialize_peer(peer, &chain_store.get_anchor_hash())?;
    }

    Ok(SelectChain::new(tree, peers))
}

pub trait PallasPoint {
    fn pallas_point(&self) -> pallas_network::miniprotocols::Point;
}

impl PallasPoint for Header {
    fn pallas_point(&self) -> pallas_network::miniprotocols::Point {
        to_pallas_point(&self.point())
    }
}

impl PallasPoint for Point {
    fn pallas_point(&self) -> pallas_network::miniprotocols::Point {
        to_pallas_point(self)
    }
}

pub fn to_pallas_point(point: &Point) -> pallas_network::miniprotocols::Point {
    match point {
        Point::Origin => pallas_network::miniprotocols::Point::Origin,
        Point::Specific(slot, hash) => {
            pallas_network::miniprotocols::Point::Specific(*slot, hash.clone())
        }
    }
}

pub trait AsTip {
    fn as_tip(&self) -> Tip;
}

impl AsTip for Header {
    fn as_tip(&self) -> Tip {
        Tip(self.pallas_point(), self.block_height())
    }
}

#[cfg(test)]
mod tests {
    use amaru_kernel::{
        EraHistory, network::NetworkName, protocol_parameters::PREPROD_INITIAL_PROTOCOL_PARAMETERS,
    };
    use amaru_stores::in_memory::MemoryStore;
    use std::path::PathBuf;
    use tokio_util::sync::CancellationToken;

    use super::{Config, StorePath, StorePath::*, bootstrap};

    #[tokio::test]
    async fn bootstrap_all_stages() {
        let network = NetworkName::Preprod;
        let era_history: &EraHistory = network.into();
        let ledger_store = MemoryStore::new(
            era_history.clone(),
            PREPROD_INITIAL_PROTOCOL_PARAMETERS.clone(),
        );

        let config = Config {
            ledger_store: InMem(ledger_store),
            chain_store: InMem(()),
            network,
            ..Config::default()
        };

        let stages = bootstrap(config, vec![], CancellationToken::new(), None)
            .await
            .unwrap();

        assert_eq!(2, stages.len());
    }

    #[test]
    fn test_store_path_display() {
        assert_eq!(format!("{}", StorePath::InMem(())), "<mem>");
        assert_eq!(
            format!(
                "{}",
                StorePath::<()>::OnDisk(PathBuf::from("/path/to/store"))
            ),
            "/path/to/store"
        );
        assert_eq!(
            format!(
                "{}",
                StorePath::<()>::OnDisk(PathBuf::from("./relative/path"))
            ),
            "./relative/path"
        );
        assert_eq!(
            format!("{}", StorePath::<()>::OnDisk(PathBuf::from(""))),
            ""
        );
    }
}
