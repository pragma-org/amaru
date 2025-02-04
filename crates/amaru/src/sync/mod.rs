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

use amaru_consensus::consensus;
use amaru_consensus::consensus::store::rocksdb::RocksDBStore;

use amaru_consensus::consensus::{
    chain_selection::{ChainSelector, ChainSelectorBuilder},
    header::{point_hash, ConwayHeader},
    store::ChainStore,
};
use amaru_ouroboros::protocol::peer::{Peer, PeerSession};
use amaru_ouroboros::protocol::Point;
use amaru_stores::rocksdb::RocksDB;
use gasket::runtime::Tether;
use opentelemetry::metrics::Counter;
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
    pub chain_database_path: PathBuf,
    pub upstream_peer: String,
    pub network_magic: u32,
    pub nonces: HashMap<Epoch, Hash<32>>,
    pub counter: Counter<u64>,
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

pub fn bootstrap(config: Config, client: &Arc<Mutex<PeerClient>>) -> miette::Result<Vec<Tether>> {
    // FIXME: Take from config / command args
    let store = RocksDB::new(&config.ledger_dir)
        .unwrap_or_else(|e| panic!("unable to open ledger store: {e:?}"));
    let (mut ledger, tip) = amaru_ledger::Stage::new(store, config.counter.clone());

    let peer_session = PeerSession {
        peer: Peer::new(&config.upstream_peer),
        peer_client: client.clone(),
    };

    let mut pull = pull::Stage::new(peer_session.clone(), vec![tip.clone()]);
    let chain_store = RocksDBStore::new(config.chain_database_path.clone())?;
    let chain_selector = make_chain_selector(tip, &chain_store, &[&peer_session]);
    let mut consensus = consensus::Stage::new(
        peer_session,
        ledger.state.clone(),
        Arc::new(Mutex::new(chain_store)),
        chain_selector,
        config.nonces,
    );

    let (to_header_validation, from_pull) = gasket::messaging::tokio::mpsc_channel(50);
    let (to_ledger, from_header_validation) = gasket::messaging::tokio::mpsc_channel(50);

    pull.downstream.connect(to_header_validation);
    consensus.upstream.connect(from_pull);
    consensus.downstream.connect(to_ledger);
    ledger.upstream.connect(from_header_validation);

    let policy = define_gasket_policy();

    let pull = gasket::runtime::spawn_stage(pull, policy.clone());
    let header_validation = gasket::runtime::spawn_stage(consensus, policy.clone());
    let ledger = gasket::runtime::spawn_stage(ledger, policy.clone());

    Ok(vec![pull, header_validation, ledger])
}

fn make_chain_selector(
    tip: Point,
    chain_store: &impl ChainStore<ConwayHeader>,
    peers: &[&PeerSession],
) -> Arc<Mutex<ChainSelector<ConwayHeader>>> {
    let mut builder = ChainSelectorBuilder::new();

    match chain_store.get(&point_hash(&tip)) {
        None => panic!("Tip {:?} not found in chain store", tip),
        Some(header) => builder.set_tip(&header),
    };

    for peer in peers {
        builder.add_peer(&peer.peer);
    }

    Arc::new(Mutex::new(builder.build()))
}
