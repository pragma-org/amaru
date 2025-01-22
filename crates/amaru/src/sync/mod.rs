use crate::{
    consensus::{
        self,
        chain_selection::{ChainSelector, ChainSelectorBuilder},
        header::{point_hash, ConwayHeader},
        peer::PeerSession,
        store::{ChainStore, SimpleChainStore},
        Peer,
    },
    ledger,
};
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
    RollForward(Peer, Point, RawHeader),
    Rollback(Peer, Point),
}

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
    let (mut ledger, tip) = ledger::Stage::new(&config.ledger_dir, config.counter.clone());
    let peer_session = PeerSession {
        peer: Peer::new(&config.upstream_peer),
        peer_client: client.clone(),
    };

    let mut pull = pull::Stage::new(peer_session.clone(), vec![tip.clone()]);
    let chain_store = SimpleChainStore::new(config.chain_database_path.clone());
    let chain_selector = make_chain_selector(tip, &chain_store, &[&peer_session]);
    let mut consensus = consensus::Stage::new(
        peer_session,
        ledger.state.clone(),
        Arc::new(Mutex::new(chain_store.clone())),
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
    chain_store: &SimpleChainStore,
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
