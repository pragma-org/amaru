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

use amaru_consensus::{
    consensus::{
        chain_selection::{ChainSelector, ChainSelectorBuilder},
        select_chain::SelectChain,
        store::ChainStore,
        store_block::StoreBlock,
        store_header::StoreHeader,
        validate_header::ValidateHeader,
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
use amaru_ledger::store::in_memory::MemoryStore;
use amaru_stores::rocksdb::{
    consensus::{InMemConsensusStore, RocksDBStore},
    RocksDB, RocksDBHistoricalStores,
};
use consensus::{
    fetch_block::BlockFetchStage, forward_chain::ForwardChainStage,
    receive_header::ReceiveHeaderStage, select_chain::SelectChainStage,
    store_block::StoreBlockStage, store_header::StoreHeaderStage,
    validate_header::ValidateHeaderStage,
};
use gasket::{
    messaging::{tokio::funnel_ports, OutputPort},
    runtime::{self, spawn_stage, Tether},
};
use ledger::ValidateBlockStage;
use pallas_network::{facades::PeerClient, miniprotocols::chainsync::Tip};
use std::{error::Error, path::PathBuf, sync::Arc};
use tokio::sync::Mutex;

pub mod common;
pub mod consensus;
pub mod ledger;
pub mod pull;

pub type BlockHash = pallas_crypto::hash::Hash<32>;

/// Whether or not data is stored on disk or in memory.
#[derive(Clone, Debug)]
pub enum StorePath {
    InMem,
    OnDisk(PathBuf),
}

pub struct Config {
    pub ledger_store: StorePath,
    pub chain_store: StorePath,
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
    let (mut ledger_stage, tip) = make_ledger(&config, era_history, global_parameters)?;

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
        .map(|session| pull::Stage::new(session.clone(), vec![tip.clone()]))
        .collect::<Vec<_>>();

    let (our_tip, header, chain_store_ref) = make_chain_store(&config, era_history, tip)?;

    let chain_selector = make_chain_selector(&header, &peer_sessions)?;
    let consensus = match ledger_stage {
        LedgerStage::InMemLedgerStage(ref validate_block_stage) => ValidateHeader::new(
            Arc::new(validate_block_stage.state.view_stake_distribution()),
            chain_store_ref.clone(),
        ),

        LedgerStage::OnDiskLedgerStage(ref validate_block_stage) => ValidateHeader::new(
            Arc::new(validate_block_stage.state.view_stake_distribution()),
            chain_store_ref.clone(),
        ),
    };

    let mut receive_header_stage = ReceiveHeaderStage::default();

    let mut validate_header_stage = ValidateHeaderStage::new(consensus, global_parameters);

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

    let (to_validate_header, from_receive_header) = gasket::messaging::tokio::mpsc_channel(50);
    let (to_store_header, from_validate_header) = gasket::messaging::tokio::mpsc_channel(50);
    let (to_select_chain, from_store_header) = gasket::messaging::tokio::mpsc_channel(50);
    let (to_fetch_block, from_select_chain) = gasket::messaging::tokio::mpsc_channel(50);
    let (to_store_block, from_fetch_block) = gasket::messaging::tokio::mpsc_channel(50);
    let (to_ledger, from_store_block) = gasket::messaging::tokio::mpsc_channel(50);
    let (to_block_forward, from_ledger) = gasket::messaging::tokio::mpsc_channel(50);

    let outputs: Vec<&mut OutputPort<ChainSyncEvent>> = stages
        .iter_mut()
        .map(|p| &mut p.downstream)
        .collect::<Vec<_>>();
    funnel_ports(outputs, &mut receive_header_stage.upstream, 50);
    receive_header_stage.downstream.connect(to_validate_header);

    validate_header_stage.upstream.connect(from_receive_header);
    validate_header_stage.downstream.connect(to_store_header);

    store_header_stage.upstream.connect(from_validate_header);
    store_header_stage.downstream.connect(to_select_chain);

    select_chain_stage.upstream.connect(from_store_header);
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

    let validate_header = gasket::runtime::spawn_stage(validate_header_stage, policy.clone());
    let receive_header = gasket::runtime::spawn_stage(receive_header_stage, policy.clone());
    let store_header = gasket::runtime::spawn_stage(store_header_stage, policy.clone());
    let select_chain = gasket::runtime::spawn_stage(select_chain_stage, policy.clone());
    let fetch = gasket::runtime::spawn_stage(fetch_block_stage, policy.clone());
    let store_block = gasket::runtime::spawn_stage(store_block_stage, policy.clone());
    let ledger = ledger_stage.spawn(policy.clone());
    let block_forward = gasket::runtime::spawn_stage(forward_chain_stage, policy.clone());

    stages.push(store_header);
    stages.push(receive_header);
    stages.push(select_chain);
    stages.push(validate_header);
    stages.push(store_block);
    stages.push(fetch);
    stages.push(ledger);
    stages.push(block_forward);
    Ok(stages)
}

type ChainStoreResult = (Tip, Option<Header>, Arc<Mutex<dyn ChainStore<Header>>>);

#[allow(clippy::todo, clippy::panic)]
fn make_chain_store(
    config: &Config,
    era_history: &EraHistory,
    tip: amaru_kernel::Point,
) -> Result<ChainStoreResult, Box<dyn Error>> {
    let chain_store: Box<dyn ChainStore<Header>> = match config.chain_store {
        StorePath::InMem => Box::new(InMemConsensusStore::new()),
        StorePath::OnDisk(ref chain_dir) => Box::new(RocksDBStore::new(chain_dir, era_history)?),
    };

    let (our_tip, header) = if let amaru_kernel::Point::Specific(_slot, hash) = &tip {
        #[allow(clippy::expect_used)]
        let header: Header = chain_store
            .load_header(&Hash::from(&**hash))
            .expect("Tip not found");
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
    era_history: &EraHistory,
    global_parameters: &GlobalParameters,
) -> Result<(LedgerStage, amaru_kernel::Point), Box<dyn std::error::Error>> {
    match config.ledger_store {
        StorePath::InMem => {
            let (ledger, tip) = ledger::ValidateBlockStage::new(
                MemoryStore {},
                MemoryStore {},
                era_history.clone(),
                global_parameters.clone(),
            )?;
            Ok((LedgerStage::InMemLedgerStage(ledger), tip))
        }
        StorePath::OnDisk(ref ledger_dir) => {
            let (ledger, tip) = ledger::ValidateBlockStage::new(
                RocksDB::new(ledger_dir, era_history)?,
                RocksDBHistoricalStores::new(ledger_dir),
                era_history.clone(),
                global_parameters.clone(),
            )?;
            Ok((LedgerStage::OnDiskLedgerStage(ledger), tip))
        }
    }
}

fn make_chain_selector(
    header: &Option<Header>,
    peers: &Vec<PeerSession>,
) -> Result<Arc<Mutex<ChainSelector<Header>>>, ConsensusError> {
    let mut builder = ChainSelectorBuilder::new();

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

    use super::{bootstrap, Config, StorePath::*};

    #[test]
    fn bootstrap_all_stages() {
        let config = Config {
            ledger_store: InMem,
            chain_store: InMem,
            ..Config::default()
        };

        let stages = bootstrap(config, vec![]).unwrap();

        assert_eq!(8, stages.len());
    }
}
