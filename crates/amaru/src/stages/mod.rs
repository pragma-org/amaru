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

use crate::stages::pure_stage_util::{PureStageSim, RecvAdapter, SendAdapter};
use amaru_consensus::{
    consensus::{
        chain_selection::{ChainSelector, ChainSelectorBuilder},
        select_chain::SelectChain,
        store::ChainStore,
        store_block::StoreBlock,
        store_header::StoreHeader,
        validate_header::{self, ValidateHeader},
        ChainSyncEvent,
    },
    peer::Peer,
    ConsensusError, IsHeader,
};
use amaru_kernel::{
    block::{BlockValidationResult, ValidateBlockEvent},
    network::NetworkName,
    protocol_parameters::GlobalParameters,
    EraHistory, Hash, Header,
};
use amaru_stores::{
    in_memory::MemoryStore,
    rocksdb::{
        consensus::{InMemConsensusStore, RocksDBStore},
        RocksDB, RocksDBHistoricalStores,
    },
};
use anyhow::Context;
use consensus::{
    fetch_block::BlockFetchStage, forward_chain::ForwardChainStage,
    receive_header::ReceiveHeaderStage, select_chain::SelectChainStage,
    store_block::StoreBlockStage, store_header::StoreHeaderStage,
};
use gasket::{
    messaging::{tokio::funnel_ports, OutputPort},
    runtime::{self, spawn_stage, Tether},
};
use ledger::ValidateBlockStage;
use pallas_network::{facades::PeerClient, miniprotocols::chainsync::Tip};
use pure_stage::{tokio::TokioBuilder, StageGraph};
use std::{
    error::Error,
    fmt::Display,
    path::PathBuf,
    sync::{Arc, RwLock},
};
use tokio::sync::Mutex;

pub mod common;
pub mod consensus;
pub mod ledger;
pub mod pull;
mod pure_stage_util;

pub type BlockHash = pallas_crypto::hash::Hash<32>;

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
        }
    }
}

/// A session with a peer, including the peer itself and a client to communicate with it.
#[derive(Clone)]
pub struct PeerSession {
    pub peer: Peer,
    pub peer_client: Arc<Mutex<PeerClient>>,
}

impl PeerSession {
    pub async fn lock(&mut self) -> tokio::sync::MutexGuard<'_, PeerClient> {
        self.peer_client.lock().await
    }
}

#[allow(clippy::todo)]
pub fn bootstrap(
    config: Config,
    clients: Vec<(String, Arc<Mutex<PeerClient>>)>,
) -> Result<Vec<Tether>, Box<dyn std::error::Error>> {
    let era_history: &EraHistory = config.network.into();

    let global_parameters: &GlobalParameters = config.network.into();

    let is_catching_up = Arc::new(RwLock::new(true));

    let (mut ledger_stage, tip) = make_ledger(
        &config,
        config.network,
        era_history.clone(),
        global_parameters.clone(),
        is_catching_up.clone(),
    )?;

    let peer_sessions: Vec<PeerSession> = clients
        .iter()
        .map(|(peer_name, client)| PeerSession {
            peer: Peer::new(peer_name),
            peer_client: client.clone(),
        })
        .collect();

    let mut fetch_block_stage = BlockFetchStage::new(peer_sessions.as_slice());

    let mut stages = peer_sessions
        .iter()
        .map(|session| pull::Stage::new(session.clone(), vec![tip.clone()], is_catching_up.clone()))
        .collect::<Vec<_>>();

    let (our_tip, header, chain_store_ref) = make_chain_store(&config, era_history, tip)?;

    let chain_selector = make_chain_selector(
        &header,
        &peer_sessions,
        global_parameters.consensus_security_param,
    )?;

    let consensus = match ledger_stage {
        LedgerStage::InMemLedgerStage(ref validate_block_stage) => ValidateHeader::new(Arc::new(
            validate_block_stage.state.view_stake_distribution(),
        )),

        LedgerStage::OnDiskLedgerStage(ref validate_block_stage) => ValidateHeader::new(Arc::new(
            validate_block_stage.state.view_stake_distribution(),
        )),
    };

    let mut receive_header_stage = ReceiveHeaderStage::default();

    let mut store_header_stage = StoreHeaderStage::new(StoreHeader::new(chain_store_ref.clone()));

    let mut select_chain_stage = SelectChainStage::new(SelectChain::new(chain_selector));

    let mut store_block_stage = StoreBlockStage::new(StoreBlock::new(chain_store_ref.clone()));

    let mut forward_chain_stage = ForwardChainStage::new(
        None,
        chain_store_ref.clone(),
        config.network_magic as u64,
        &config.listen_address,
        config.max_downstream_peers,
        our_tip,
    );

    let (to_store_header, from_receive_header) = gasket::messaging::tokio::mpsc_channel(50);
    let (to_fetch_block, from_select_chain) = gasket::messaging::tokio::mpsc_channel(50);
    let (to_store_block, from_fetch_block) = gasket::messaging::tokio::mpsc_channel(50);
    let (to_ledger, from_store_block) = gasket::messaging::tokio::mpsc_channel(50);
    let (to_block_forward, from_ledger) = gasket::messaging::tokio::mpsc_channel(50);

    // start pure-stage parts, whose lifecycle is managed by a single gasket stage
    let mut network = TokioBuilder::default();

    let (network_output, validate_header_input) =
        build_stage_graph(global_parameters, consensus, &mut network);

    network.resources().put(chain_store_ref);
    network.resources().put(global_parameters);

    let rt = tokio::runtime::Runtime::new().context("starting tokio runtime for pure_stages")?;
    let network = network.run(rt.handle().clone());
    let pure_stages = PureStageSim::new(network, rt);

    let outputs: Vec<&mut OutputPort<ChainSyncEvent>> = stages
        .iter_mut()
        .map(|p| &mut p.downstream)
        .collect::<Vec<_>>();
    funnel_ports(outputs, &mut receive_header_stage.upstream, 50);
    receive_header_stage.downstream.connect(to_store_header);

    store_header_stage.upstream.connect(from_receive_header);
    store_header_stage
        .downstream
        .connect(SendAdapter(validate_header_input));

    select_chain_stage
        .upstream
        .connect(RecvAdapter(network_output));
    select_chain_stage.downstream.connect(to_fetch_block);

    fetch_block_stage.upstream.connect(from_select_chain);
    fetch_block_stage.downstream.connect(to_store_block);

    store_block_stage.upstream.connect(from_fetch_block);
    store_block_stage.downstream.connect(to_ledger);

    ledger_stage.connect(from_store_block, to_block_forward);

    forward_chain_stage.upstream.connect(from_ledger);

    // No retry, crash on panics.
    let policy = runtime::Policy::default();

    let mut stages = stages
        .into_iter()
        .map(|p| spawn_stage(p, policy.clone()))
        .collect::<Vec<_>>();

    let pure_stages = gasket::runtime::spawn_stage(pure_stages, policy.clone());

    let receive_header = gasket::runtime::spawn_stage(receive_header_stage, policy.clone());
    let store_header = gasket::runtime::spawn_stage(store_header_stage, policy.clone());
    let select_chain = gasket::runtime::spawn_stage(select_chain_stage, policy.clone());
    let fetch = gasket::runtime::spawn_stage(fetch_block_stage, policy.clone());
    let store_block = gasket::runtime::spawn_stage(store_block_stage, policy.clone());
    let ledger = ledger_stage.spawn(policy.clone());
    let block_forward = gasket::runtime::spawn_stage(forward_chain_stage, policy.clone());

    stages.push(pure_stages);

    stages.push(store_header);
    stages.push(receive_header);
    stages.push(select_chain);
    stages.push(store_block);
    stages.push(fetch);
    stages.push(ledger);
    stages.push(block_forward);
    Ok(stages)
}

fn build_stage_graph(
    global_parameters: &GlobalParameters,
    consensus: ValidateHeader,
    network: &mut impl StageGraph,
) -> (
    pure_stage::Receiver<amaru_consensus::consensus::DecodedChainSyncEvent>,
    pure_stage::Sender<amaru_consensus::consensus::DecodedChainSyncEvent>,
) {
    let validate_header_stage = network.stage("validate_header", validate_header::stage);

    let (network_output_ref, network_output) = network.output("network_output", 50);

    let validate_header_stage = network.wire_up(
        validate_header_stage,
        (consensus, global_parameters.clone(), network_output_ref),
    );

    let validate_header_input = network.input(&validate_header_stage);
    (network_output, validate_header_input)
}

type ChainStoreResult = (Tip, Option<Header>, Arc<Mutex<dyn ChainStore<Header>>>);

#[allow(clippy::todo, clippy::panic)]
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
        #[allow(clippy::expect_used)]
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
    InMemLedgerStage(ValidateBlockStage<MemoryStore, MemoryStore>),
    OnDiskLedgerStage(ValidateBlockStage<RocksDB, RocksDBHistoricalStores>),
}

impl LedgerStage {
    fn spawn(self, policy: runtime::Policy) -> Tether {
        match self {
            LedgerStage::InMemLedgerStage(validate_block_stage) => {
                spawn_stage(validate_block_stage, policy)
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
    is_catching_up: Arc<RwLock<bool>>,
) -> Result<(LedgerStage, amaru_kernel::Point), Box<dyn std::error::Error>> {
    match &config.ledger_store {
        StorePath::InMem(store) => {
            let (ledger, tip) = ledger::ValidateBlockStage::new(
                store.clone(),
                store.clone(),
                network,
                era_history,
                global_parameters,
                is_catching_up,
            )?;
            Ok((LedgerStage::InMemLedgerStage(ledger), tip))
        }
        StorePath::OnDisk(ref ledger_dir) => {
            let (ledger, tip) = ledger::ValidateBlockStage::new(
                RocksDB::new(ledger_dir)?,
                RocksDBHistoricalStores::new(ledger_dir),
                network,
                era_history,
                global_parameters,
                is_catching_up,
            )?;
            Ok((LedgerStage::OnDiskLedgerStage(ledger), tip))
        }
    }
}

fn make_chain_selector(
    header: &Option<Header>,
    peers: &Vec<PeerSession>,
    consensus_security_parameter: usize,
) -> Result<Arc<Mutex<ChainSelector<Header>>>, ConsensusError> {
    let mut builder = ChainSelectorBuilder::new();

    builder.set_max_fragment_length(consensus_security_parameter);

    match header {
        Some(h) => builder.set_tip(h),
        None => &builder,
    };

    for peer in peers {
        builder.add_peer(&peer.peer);
    }

    Ok(Arc::new(Mutex::new(builder.build()?)))
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
        network::NetworkName, protocol_parameters::PREPROD_INITIAL_PROTOCOL_PARAMETERS, EraHistory,
        PROTOCOL_VERSION_9,
    };
    use amaru_ledger::store::{Store, TransactionalContext};
    use amaru_stores::in_memory::MemoryStore;
    use std::path::PathBuf;

    use super::{bootstrap, Config, StorePath, StorePath::*};

    #[test]
    fn bootstrap_all_stages() {
        let network = NetworkName::Preprod;
        let era_history: &EraHistory = network.into();
        let ledger_store = MemoryStore::new(era_history.clone(), PROTOCOL_VERSION_9);

        // Add initial protocol parameters to the database; needed by the ledger.
        let transaction = ledger_store.create_transaction();
        transaction
            .set_protocol_parameters(&PREPROD_INITIAL_PROTOCOL_PARAMETERS)
            .unwrap();
        transaction.commit().unwrap();

        let config = Config {
            ledger_store: InMem(ledger_store),
            chain_store: InMem(()),
            network,
            ..Config::default()
        };

        let stages = bootstrap(config, vec![]).unwrap();

        assert_eq!(8, stages.len());
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
