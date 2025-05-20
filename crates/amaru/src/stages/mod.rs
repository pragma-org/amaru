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
    network::NetworkName, protocol_parameters::GlobalParameters, EraHistory, Hash, Header,
};
use amaru_stores::rocksdb::{consensus::RocksDBStore, RocksDB, RocksDBHistoricalStores};
use consensus::{
    fetch_block::BlockFetchStage, forward_chain::ForwardChainStage,
    receive_header::ReceiveHeaderStage, select_chain::SelectChainStage,
    store_block::StoreBlockStage, store_header::StoreHeaderStage,
    validate_header::ValidateHeaderStage,
};
use gasket::{
    messaging::{tokio::funnel_ports, OutputPort},
    runtime::Tether,
};
use pallas_network::{
    facades::PeerClient,
    miniprotocols::{chainsync::Tip, Point},
};
use std::{path::PathBuf, sync::Arc};
use tokio::sync::Mutex;

pub mod consensus;
pub mod ledger;
pub mod pull;

pub type BlockHash = pallas_crypto::hash::Hash<32>;

pub struct Config {
    pub ledger_dir: PathBuf,
    pub chain_dir: PathBuf,
    pub upstream_peers: Vec<String>,
    pub network: NetworkName,
    pub network_magic: u32,
    pub listen_address: String,
    pub max_downstream_peers: usize,
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

pub fn bootstrap(
    config: Config,
    clients: Vec<(String, Arc<Mutex<PeerClient>>)>,
) -> Result<Vec<Tether>, Box<dyn std::error::Error>> {
    // FIXME: Take from config / command args
    let era_history: &EraHistory = config.network.into();
    let store = RocksDB::new(&config.ledger_dir, era_history)?;
    let snapshots = RocksDBHistoricalStores::new(&config.ledger_dir);
    let global_parameters = GlobalParameters::default();
    let (mut ledger, tip) = ledger::ValidateBlockStage::new(
        store,
        snapshots,
        era_history.clone(),
        global_parameters.clone(),
    )?;

    let peer_sessions: Vec<PeerSession> = clients
        .iter()
        .map(|(peer_name, client)| PeerSession {
            peer: Peer::new(peer_name),
            peer_client: client.clone(),
        })
        .collect();

    let mut fetch_block_stage = BlockFetchStage::new(peer_sessions.as_slice());

    let mut pulls = peer_sessions
        .iter()
        .map(|session| pull::Stage::new(session.clone(), vec![tip.clone()]))
        .collect::<Vec<_>>();
    let chain_store = RocksDBStore::new(config.chain_dir.clone(), era_history)?;

    let our_tip = if let amaru_kernel::Point::Specific(_slot, hash) = &tip {
        #[allow(clippy::expect_used)]
        let header: Header = chain_store
            .load_header(&Hash::from(&**hash))
            .expect("Tip not found");
        Tip(header.pallas_point(), header.block_height())
    } else {
        Tip(Point::Origin, 0)
    };

    let chain_selector = make_chain_selector(tip.clone(), &chain_store, &peer_sessions)?;
    let chain_store_ref = Arc::new(Mutex::new(chain_store));
    let consensus = ValidateHeader::new(
        Box::new(ledger.state.view_stake_distribution()),
        chain_store_ref.clone(),
    );

    let mut receive_header_stage = ReceiveHeaderStage::default();

    let mut validate_header_stage = ValidateHeaderStage::new(consensus, &global_parameters);

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

    let outputs: Vec<&mut OutputPort<ChainSyncEvent>> = pulls
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

    ledger.upstream.connect(from_store_block);
    ledger.downstream.connect(to_block_forward);

    forward_chain_stage.upstream.connect(from_ledger);

    // No retry, crash on panics.
    let policy = gasket::runtime::Policy::default();

    let mut pulls = pulls
        .into_iter()
        .map(|p| gasket::runtime::spawn_stage(p, policy.clone()))
        .collect::<Vec<_>>();

    let validate_header = gasket::runtime::spawn_stage(validate_header_stage, policy.clone());
    let receive_header = gasket::runtime::spawn_stage(receive_header_stage, policy.clone());
    let store_header = gasket::runtime::spawn_stage(store_header_stage, policy.clone());
    let select_chain = gasket::runtime::spawn_stage(select_chain_stage, policy.clone());
    let fetch = gasket::runtime::spawn_stage(fetch_block_stage, policy.clone());
    let ledger = gasket::runtime::spawn_stage(ledger, policy.clone());
    let block_forward = gasket::runtime::spawn_stage(forward_chain_stage, policy.clone());

    pulls.push(store_header);
    pulls.push(receive_header);
    pulls.push(select_chain);
    pulls.push(validate_header);
    pulls.push(fetch);
    pulls.push(ledger);
    pulls.push(block_forward);
    Ok(pulls)
}

fn make_chain_selector(
    tip: amaru_kernel::Point,
    chain_store: &impl ChainStore<Header>,
    peers: &Vec<PeerSession>,
) -> Result<Arc<Mutex<ChainSelector<Header>>>, ConsensusError> {
    let mut builder = ChainSelectorBuilder::new();

    #[allow(clippy::panic)]
    match chain_store.load_header(&Hash::from(&tip)) {
        None => panic!("Tip {:?} not found in chain store", tip),
        Some(header) => builder.set_tip(&header),
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
