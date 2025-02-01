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

use crate::consensus::{self, peer::PeerSession, Peer};

use amaru_stores::rocksdb::RocksDB;
use gasket::runtime::Tether;
use opentelemetry::metrics::Counter;
use pallas_crypto::hash::Hash;
use pallas_network::facades::PeerClient;
use pallas_primitives::conway::Epoch;
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio::sync::Mutex;

mod fetch;
mod pull;

pub type Slot = u64;
pub type BlockHash = pallas_crypto::hash::Hash<32>;
pub type RawHeader = Vec<u8>;
pub type Point = pallas_network::miniprotocols::Point;

#[derive(Clone)]
pub enum PullEvent {
    RollForward(Point, RawHeader),
    Rollback(Point),
}

pub struct Config {
    pub ledger_dir: PathBuf,
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

    let mut pull = pull::Stage::new(client.clone(), vec![tip]);
    let peer_session = PeerSession {
        peer: Peer::new(&config.upstream_peer),
        peer_client: client.clone(),
    };
    let mut header_validation =
        consensus::Stage::new(peer_session, ledger.state.clone(), config.nonces);

    let (to_header_validation, from_pull) = gasket::messaging::tokio::mpsc_channel(50);
    let (to_ledger, from_header_validation) = gasket::messaging::tokio::mpsc_channel(50);

    pull.downstream.connect(to_header_validation);
    header_validation.upstream.connect(from_pull);
    header_validation.downstream.connect(to_ledger);
    ledger.upstream.connect(from_header_validation);

    let policy = define_gasket_policy();

    let pull = gasket::runtime::spawn_stage(pull, policy.clone());
    let header_validation = gasket::runtime::spawn_stage(header_validation, policy.clone());
    let ledger = gasket::runtime::spawn_stage(ledger, policy.clone());

    Ok(vec![pull, header_validation, ledger])
}
