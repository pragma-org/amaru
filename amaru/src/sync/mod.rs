use crate::{consensus, ledger};
use gasket::runtime::Tether;
use pallas_network::facades::PeerClient;
use std::sync::{Arc, Mutex};

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

pub fn bootstrap(config: Config, client: &Arc<Mutex<PeerClient>>) -> miette::Result<Vec<Tether>> {
    let mut pull = pull::Stage::new(client.clone(), config.intersection.clone());
    let mut header_validation = consensus::header_validation::Stage::new(client.clone());
    let mut ledger = ledger::worker::Stage::new();

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
