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
    chain_forward,
    consensus::{
        chain_selection::{ChainSelector, ChainSelectorBuilder},
        fetch::BlockFetchStage,
        header_validation::Consensus,
        store::{rocksdb::RocksDBStore, ChainStore},
        wiring::{HeaderStage, PullEvent},
    },
    peer::{Peer, PeerSession},
    ConsensusError,
};
use amaru_kernel::{Hash, Header, Point};
use amaru_stores::rocksdb::RocksDB;
use gasket::{
    messaging::{tokio::funnel_ports, OutputPort},
    runtime::Tether,
};
use pallas_network::facades::PeerClient;
use std::{path::PathBuf, sync::Arc};
use tokio::sync::Mutex;

use crate::pipeline::Stage;

pub mod pull;

pub type Slot = u64;
pub type BlockHash = pallas_crypto::hash::Hash<32>;

pub struct Config {
    pub ledger_dir: PathBuf,
    pub chain_dir: PathBuf,
    pub upstream_peers: Vec<String>,
    pub network_magic: u32,
}

pub fn bootstrap(
    config: Config,
    clients: Vec<(String, Arc<Mutex<PeerClient>>)>,
) -> Result<Vec<Tether>, Box<dyn std::error::Error>> {
    // FIXME: Take from config / command args
    let store = RocksDB::new(&config.ledger_dir)?;
    let (mut ledger, tip) = Stage::new(store);

    let peer_sessions: Vec<PeerSession> = clients
        .iter()
        .map(|(peer_name, client)| PeerSession {
            peer: Peer::new(peer_name),
            peer_client: client.clone(),
        })
        .collect();

    let mut block_fetch_stage = BlockFetchStage::new(peer_sessions.as_slice());

    let mut pulls = peer_sessions
        .iter()
        .map(|session| pull::Stage::new(session.clone(), vec![tip.clone()]))
        .collect::<Vec<_>>();
    let chain_store = RocksDBStore::new(config.chain_dir.clone())?;
    let chain_selector = make_chain_selector(tip, &chain_store, &peer_sessions)?;
    let chain_ref = Arc::new(Mutex::new(chain_store));
    let consensus = Consensus::new(
        Box::new(ledger.state.view_stake_distribution()),
        chain_ref.clone(),
        chain_selector,
    );

    let mut consensus_stage = HeaderStage::new(consensus);
    let mut block_forward = chain_forward::ForwardStage::new(chain_ref.clone());

    let (to_block_fetch, from_consensus_stage) = gasket::messaging::tokio::mpsc_channel(50);
    let (to_ledger, from_block_fetch) = gasket::messaging::tokio::mpsc_channel(50);
    let (to_block_forward, from_ledger) = gasket::messaging::tokio::mpsc_channel(50);

    let outputs: Vec<&mut OutputPort<PullEvent>> = pulls
        .iter_mut()
        .map(|p| &mut p.downstream)
        .collect::<Vec<_>>();
    funnel_ports(outputs, &mut consensus_stage.upstream, 50);
    consensus_stage.downstream.connect(to_block_fetch);
    block_fetch_stage.upstream.connect(from_consensus_stage);
    block_fetch_stage.downstream.connect(to_ledger);
    ledger.upstream.connect(from_block_fetch);
    ledger.downstream.connect(to_block_forward);
    block_forward.upstream.connect(from_ledger);

    // No retry, crash on panics.
    let policy = gasket::runtime::Policy::default();

    let mut pulls = pulls
        .into_iter()
        .map(|p| gasket::runtime::spawn_stage(p, policy.clone()))
        .collect::<Vec<_>>();

    let header_validation = gasket::runtime::spawn_stage(consensus_stage, policy.clone());
    let fetch = gasket::runtime::spawn_stage(block_fetch_stage, policy.clone());
    let ledger = gasket::runtime::spawn_stage(ledger, policy.clone());
    let block_forward = gasket::runtime::spawn_stage(block_forward, policy.clone());

    pulls.push(header_validation);
    pulls.push(fetch);
    pulls.push(ledger);
    pulls.push(block_forward);
    Ok(pulls)
}

fn make_chain_selector(
    tip: Point,
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
