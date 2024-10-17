use crate::consensus::validate;
use crate::ledger;
use gasket::framework::AsWorkError;
use gasket::runtime::Tether;
use pallas_network::facades::PeerClient;
use std::sync::{Arc, Mutex};
use tracing::debug;

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
    pub upstream_peer: String,
    pub network_magic: u32,
    pub intersection: Vec<Point>,
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

pub fn bootstrap(config: Config) -> miette::Result<Vec<Tether>> {
    // FIXME: this is a hack to get around the fact that PeerClient::connect is async
    let peer_session = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            debug!("connecting to peer");
            PeerClient::connect(config.upstream_peer.clone(), config.network_magic as u64).await
        })
    })
    .or_retry()
    .unwrap(); // fixme: unwrap

    let peer_session = Arc::new(Mutex::new(peer_session));
    let mut pull = pull::Stage::new(Arc::clone(&peer_session), config.intersection.clone());

    let mut validate = validate::Stage::new(Arc::clone(&peer_session));

    let mut ledger = ledger::worker::Stage::new();

    let (to_validate, from_pull) = gasket::messaging::tokio::mpsc_channel(50);
    let (to_ledger, from_validate) = gasket::messaging::tokio::mpsc_channel(50);
    pull.downstream.connect(to_validate);
    validate.upstream.connect(from_pull);
    validate.downstream.connect(to_ledger);
    ledger.upstream.connect(from_validate);

    let policy = define_gasket_policy();

    let pull = gasket::runtime::spawn_stage(pull, policy.clone());
    let validate = gasket::runtime::spawn_stage(validate, policy.clone());
    let ledger = gasket::runtime::spawn_stage(ledger, policy.clone());

    Ok(vec![pull, validate, ledger])
}
