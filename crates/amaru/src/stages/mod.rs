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

use crate::stages::consensus::clients_block_fetcher::ClientsBlockFetcher;
use crate::stages::pure_stage_util::{PureStageSim, RecvAdapter, SendAdapter};
use amaru_consensus::consensus::block_effects;
use amaru_consensus::{
    ConsensusError, IsHeader,
    consensus::{
        ChainSyncEvent, build_stage_graph, headers_tree::HeadersTree, select_chain::SelectChain,
        store::ChainStore, store_block::StoreBlock, store_effects, validate_header::ValidateHeader,
    },
};
use amaru_kernel::{
    EraHistory, Hash, Header, Point,
    block::{BlockValidationResult, ValidateBlockEvent},
    network::NetworkName,
    peer::Peer,
    protocol_parameters::GlobalParameters,
};
use amaru_stores::{
    in_memory::MemoryStore,
    rocksdb::{
        RocksDB, RocksDBHistoricalStores,
        consensus::{InMemConsensusStore, RocksDBStore},
    },
};
use anyhow::Context;
use consensus::{forward_chain::ForwardChainStage, store_block::StoreBlockStage};
use gasket::{
    messaging::OutputPort,
    runtime::{self, Tether, spawn_stage},
};
use ledger::ValidateBlockStage;
use pallas_network::{
    facades::PeerClient,
    miniprotocols::{
        blockfetch,
        chainsync::{Client, HeaderContent, Tip},
    },
};
use pure_stage::{StageGraph, tokio::TokioBuilder};
use std::{error::Error, fmt::Display, path::PathBuf, sync::Arc};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub mod common;
pub mod consensus;
pub mod ledger;
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
            chain_store: StorePath::OnDisk(PathBuf::from("./chain.db.1")),
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

pub fn bootstrap(
    config: Config,
    clients: Vec<(String, PeerClient)>,
    exit: CancellationToken,
) -> Result<Vec<Tether>, Box<dyn Error>> {
    let era_history: &EraHistory = config.network.into();

    let global_parameters: &GlobalParameters = config.network.into();

    let peers: Vec<Peer> = clients.iter().map(|c| Peer::new(&c.0)).collect();

    let (mut ledger_stage, tip) = make_ledger(
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
        Vec<(Peer, blockfetch::Client)>,
    ) = clients
        .into_iter()
        .map(|(peer_name, client)| {
            let PeerClient {
                chainsync,
                blockfetch,
                ..
            } = client;
            (
                (Peer::new(&peer_name), chainsync),
                (Peer::new(&peer_name), blockfetch),
            )
        })
        .collect();

    let mut stages = chain_syncs
        .into_iter()
        .map(|session| pull::Stage::new(session.0, session.1, vec![tip.clone()]))
        .collect::<Vec<_>>();

    let (our_tip, header, chain_store_ref) = make_chain_store(&config, era_history, tip)?;

    let chain_selector =
        make_chain_selector(header, &peers, global_parameters.consensus_security_param)?;

    let consensus = match &ledger_stage {
        LedgerStage::InMemLedgerStage(validate_block_stage) => ValidateHeader::new(Arc::new(
            validate_block_stage.state.view_stake_distribution(),
        )),

        LedgerStage::OnDiskLedgerStage(validate_block_stage) => ValidateHeader::new(Arc::new(
            validate_block_stage.state.view_stake_distribution(),
        )),
    };

    let mut store_block_stage = StoreBlockStage::new(StoreBlock::new(chain_store_ref.clone()));

    let mut forward_chain_stage = ForwardChainStage::new(
        None,
        chain_store_ref.clone(),
        config.network_magic as u64,
        &config.listen_address,
        config.max_downstream_peers,
        our_tip,
    );

    let (to_ledger, from_store_block) = gasket::messaging::tokio::mpsc_channel(50);
    let (to_block_forward, from_ledger) = gasket::messaging::tokio::mpsc_channel(50);

    // start pure-stage parts, whose lifecycle is managed by a single gasket stage
    let mut network = TokioBuilder::default();
    let (output_ref, output_stage) = network.output("output", 50);

    let graph_input = build_stage_graph(
        global_parameters,
        consensus,
        chain_selector,
        &mut network,
        output_ref,
    );
    let graph_input = network.input(&graph_input);

    network
        .resources()
        .put::<store_effects::ResourceHeaderStore>(chain_store_ref);
    network
        .resources()
        .put::<store_effects::ResourceParameters>(global_parameters.clone());

    network
        .resources()
        .put::<block_effects::ResourceBlockFetcher>(Arc::new(ClientsBlockFetcher::new(
            block_fetchs,
        )));

    let rt = tokio::runtime::Runtime::new().context("starting tokio runtime for pure_stages")?;
    let network = network.run(rt.handle().clone());
    let pure_stages = PureStageSim::new(network, rt, exit);

    let outputs: Vec<&mut OutputPort<ChainSyncEvent>> = stages
        .iter_mut()
        .map(|p| &mut p.downstream)
        .collect::<Vec<_>>();

    for output in outputs {
        output.connect(SendAdapter(graph_input.clone()));
    }

    store_block_stage
        .upstream
        .connect(RecvAdapter(output_stage));
    store_block_stage.downstream.connect(to_ledger);

    ledger_stage.connect(from_store_block, to_block_forward);

    forward_chain_stage.upstream.connect(from_ledger);

    // No retry, crash on panics.
    let policy = runtime::Policy::default();

    let mut stages = stages
        .into_iter()
        .map(|p| spawn_stage(p, policy.clone()))
        .collect::<Vec<_>>();

    let pure_stages = spawn_stage(pure_stages, policy.clone());

    let store_block = spawn_stage(store_block_stage, policy.clone());
    let ledger = ledger_stage.spawn(policy.clone());
    let block_forward = spawn_stage(forward_chain_stage, policy.clone());

    stages.push(pure_stages);

    stages.push(store_block);
    stages.push(ledger);
    stages.push(block_forward);
    Ok(stages)
}

type ChainStoreResult = (Tip, Option<Header>, Arc<Mutex<dyn ChainStore<Header>>>);

#[expect(clippy::panic)]
fn make_chain_store(
    config: &Config,
    era_history: &EraHistory,
    tip: amaru_kernel::Point,
) -> Result<ChainStoreResult, Box<dyn Error>> {
    let chain_store: Box<dyn ChainStore<Header>> = match config.chain_store {
        StorePath::InMem(()) => Box::new(InMemConsensusStore::new()),
        StorePath::OnDisk(ref chain_dir) => Box::new(RocksDBStore::new(chain_dir, era_history)?),
    };

    let (our_tip, header) = if let amaru_kernel::Point::Specific(_slot, hash) = &tip {
        let tip_hash = &Hash::from(&**hash);
        let header: Header = chain_store.load_header(tip_hash).unwrap_or_else(|| {
            panic!(
                "Tip {} not found in chain database '{}'",
                tip_hash, config.chain_store
            )
        });
        (
            Tip(header.pallas_point(), header.block_height()),
            Some(header),
        )
    } else {
        (Tip(pallas_network::miniprotocols::Point::Origin, 0), None)
    };

    let chain_store_ref: Arc<Mutex<dyn ChainStore<Header>>> = Arc::new(Mutex::new(chain_store));
    Ok((our_tip, header, chain_store_ref))
}

enum LedgerStage {
    InMemLedgerStage(Box<ValidateBlockStage<MemoryStore, MemoryStore>>),
    OnDiskLedgerStage(ValidateBlockStage<RocksDB, RocksDBHistoricalStores>),
}

impl LedgerStage {
    fn spawn(self, policy: runtime::Policy) -> Tether {
        match self {
            LedgerStage::InMemLedgerStage(validate_block_stage) => {
                spawn_stage(*validate_block_stage, policy)
            }
            LedgerStage::OnDiskLedgerStage(validate_block_stage) => {
                spawn_stage(validate_block_stage, policy)
            }
        }
    }

    fn connect(
        &mut self,
        from_store_block: gasket::messaging::tokio::ChannelRecvAdapter<ValidateBlockEvent>,
        to_block_forward: gasket::messaging::tokio::ChannelSendAdapter<BlockValidationResult>,
    ) {
        match self {
            LedgerStage::InMemLedgerStage(validate_block_stage) => {
                validate_block_stage.upstream.connect(from_store_block);
                validate_block_stage.downstream.connect(to_block_forward);
            }
            LedgerStage::OnDiskLedgerStage(validate_block_stage) => {
                validate_block_stage.upstream.connect(from_store_block);
                validate_block_stage.downstream.connect(to_block_forward);
            }
        }
    }
}

fn make_ledger(
    config: &Config,
    network: NetworkName,
    era_history: EraHistory,
    global_parameters: GlobalParameters,
) -> Result<(LedgerStage, amaru_kernel::Point), Box<dyn std::error::Error>> {
    match &config.ledger_store {
        StorePath::InMem(store) => {
            let (ledger, tip) = ledger::ValidateBlockStage::new(
                store.clone(),
                store.clone(),
                network,
                era_history,
                global_parameters,
            )?;
            Ok((LedgerStage::InMemLedgerStage(Box::new(ledger)), tip))
        }
        StorePath::OnDisk(ledger_dir) => {
            let (ledger, tip) = ledger::ValidateBlockStage::new(
                RocksDB::new(ledger_dir)?,
                RocksDBHistoricalStores::new(
                    ledger_dir,
                    u64::from(config.max_extra_ledger_snapshots),
                ),
                network,
                era_history,
                global_parameters,
            )?;
            Ok((LedgerStage::OnDiskLedgerStage(ledger), tip))
        }
    }
}

fn make_chain_selector(
    header: Option<Header>,
    peers: &Vec<Peer>,
    consensus_security_parameter: usize,
) -> Result<SelectChain, ConsensusError> {
    let root_hash = match &header {
        Some(h) => h.hash(),
        None => Point::Origin.hash(),
    };

    // TODO: initialize the headers tree from the ChainDB store
    let mut tree = HeadersTree::new(consensus_security_parameter, &header);

    for peer in peers {
        tree.initialize_peer(peer, &root_hash)?;
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

impl PallasPoint for amaru_kernel::Point {
    fn pallas_point(&self) -> pallas_network::miniprotocols::Point {
        to_pallas_point(self)
    }
}

fn to_pallas_point(point: &amaru_kernel::Point) -> pallas_network::miniprotocols::Point {
    match point {
        amaru_kernel::Point::Origin => pallas_network::miniprotocols::Point::Origin,
        amaru_kernel::Point::Specific(slot, hash) => {
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

    #[test]
    fn bootstrap_all_stages() {
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

        let stages = bootstrap(config, vec![], CancellationToken::new()).unwrap();

        assert_eq!(4, stages.len());
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
