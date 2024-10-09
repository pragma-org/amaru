use gasket::runtime::Tether;

///! Sync pipeline
///!
///! The sync pipeline is responsible for fetching blocks from the upstream node and
///! applying them to the local chain.
mod pull;
mod validate;

pub type Slot = u64;
pub type BlockHash = pallas_crypto::hash::Hash<32>;
pub type BlockCbor = Vec<u8>;
pub type Point = pallas_network::miniprotocols::Point;

#[derive(Clone)]
pub enum PullEvent {
    RollForward(Point, BlockCbor),
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
    let mut pull = pull::Stage::new(
        config.upstream_peer.clone(),
        config.network_magic,
        config.intersection.clone(),
    );

    let mut validate = validate::Stage::new();

    let (to_validate, from_pull) = gasket::messaging::tokio::mpsc_channel(50);
    pull.downstream.connect(to_validate);
    validate.upstream.connect(from_pull);

    let policy = define_gasket_policy();

    let pull = gasket::runtime::spawn_stage(pull, policy.clone());
    let validate = gasket::runtime::spawn_stage(validate, policy.clone());

    Ok(vec![pull, validate])
}
