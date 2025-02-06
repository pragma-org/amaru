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

use amaru_consensus::consensus::store::rocksdb::RocksDBStore;
use amaru_consensus::consensus::{
    chain_selection::{ChainSelector, ChainSelectorBuilder},
    header::{point_hash, ConwayHeader},
    store::ChainStore,
};
use amaru_consensus::{chain_forward, consensus};
use amaru_ouroboros::protocol::peer::{Peer, PeerSession};
use amaru_ouroboros::protocol::{Point, PullEvent};
use amaru_stores::rocksdb::RocksDB;
use gasket::messaging::tokio::funnel_ports;
use gasket::messaging::OutputPort;
use gasket::runtime::Tether;
use pallas_crypto::hash::Hash;
use pallas_network::facades::PeerClient;
use pallas_primitives::conway::Epoch;
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio::sync::Mutex;

pub mod fetch;
pub mod pull;

pub type Slot = u64;
pub type BlockHash = pallas_crypto::hash::Hash<32>;

pub struct Config {
    pub ledger_dir: PathBuf,
    pub chain_dir: PathBuf,
    pub upstream_peers: Vec<String>,
    pub network_magic: u32,
    pub nonces: HashMap<Epoch, Hash<32>>,
}

fn define_gasket_policy() -> gasket::runtime::Policy {
    let retries = gasket::retries::Policy {
        max_retries: 20,
        backoff_unit: std::time::Duration::from_secs(1),
        backoff_factor: 2,
        max_backoff: std::time::Duration::from_secs(60),
        dismissible: false,
    };

    gasket::runtime::Policy {
        //be generous with tick timeout to avoid timeout during block awaits
        tick_timeout: std::time::Duration::from_secs(600).into(),
        bootstrap_retry: retries.clone(),
        work_retry: retries.clone(),
        teardown_retry: retries.clone(),
    }
}

pub fn bootstrap(
    config: Config,
    clients: Vec<(String, Arc<Mutex<PeerClient>>)>,
) -> miette::Result<Vec<Tether>> {
    // FIXME: Take from config / command args
    let store = RocksDB::new(&config.ledger_dir)
        .unwrap_or_else(|e| panic!("unable to open ledger store: {e:?}"));
    let (mut ledger, tip) = amaru_ledger::Stage::new(store);

    let peer_sessions: Vec<PeerSession> = clients
        .iter()
        .map(|(peer_name, client)| PeerSession {
            peer: Peer::new(peer_name),
            peer_client: client.clone(),
        })
        .collect();

    let mut pulls = peer_sessions
        .iter()
        .map(|session| pull::Stage::new(session.clone(), vec![tip.clone()]))
        .collect::<Vec<_>>();
    let chain_store = RocksDBStore::new(config.chain_dir.clone())?;
    let chain_selector = make_chain_selector(tip, &chain_store, &peer_sessions);
    let chain_ref = Arc::new(Mutex::new(chain_store));
    let mut consensus = consensus::HeaderStage::new(
        peer_sessions,
        ledger.state.clone(),
        chain_ref.clone(),
        chain_selector,
        config.nonces,
    );

    let mut block_forward = chain_forward::ForwardStage::new(chain_ref.clone());

    let (to_ledger, from_header_validation) = gasket::messaging::tokio::mpsc_channel(50);
    let (to_block_forward, from_ledger) = gasket::messaging::tokio::mpsc_channel(50);

    let outputs: Vec<&mut OutputPort<PullEvent>> = pulls
        .iter_mut()
        .map(|p| &mut p.downstream)
        .collect::<Vec<_>>();
    funnel_ports(outputs, &mut consensus.upstream, 50);
    consensus.downstream.connect(to_ledger);
    ledger.upstream.connect(from_header_validation);
    ledger.downstream.connect(to_block_forward);
    block_forward.upstream.connect(from_ledger);

    let policy = define_gasket_policy();

    let mut pulls = pulls
        .into_iter()
        .map(|p| gasket::runtime::spawn_stage(p, policy.clone()))
        .collect::<Vec<_>>();

    let header_validation = gasket::runtime::spawn_stage(consensus, policy.clone());
    let ledger = gasket::runtime::spawn_stage(ledger, policy.clone());
    let block_forward = gasket::runtime::spawn_stage(block_forward, policy.clone());

    pulls.push(header_validation);
    pulls.push(ledger);
    pulls.push(block_forward);
    Ok(pulls)
}

fn make_chain_selector(
    tip: Point,
    chain_store: &impl ChainStore<ConwayHeader>,
    peers: &Vec<PeerSession>,
) -> Arc<Mutex<ChainSelector<ConwayHeader>>> {
    let mut builder = ChainSelectorBuilder::new();

    match chain_store.load_header(&point_hash(&tip)) {
        None => panic!("Tip {:?} not found in chain store", tip),
        Some(header) => builder.set_tip(&header),
    };

    for peer in peers {
        builder.add_peer(&peer.peer);
    }

    Arc::new(Mutex::new(builder.build()))
}
