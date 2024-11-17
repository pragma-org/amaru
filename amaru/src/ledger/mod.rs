use crate::{consensus::ValidateHeaderEvent, ledger::state::BackwardErr};
use gasket::framework::*;
use pallas_codec::minicbor as cbor;
use pallas_crypto::hash::{Hash, Hasher};
use pallas_primitives::conway::MintedBlock;
use std::path::PathBuf;
use tracing::{error, info, warn};

pub type UpstreamPort = gasket::messaging::InputPort<ValidateHeaderEvent>;

use store::Store;

pub mod stake_distribution;
pub mod stake_pools;

pub(crate) mod kernel;
pub(crate) mod mock;
pub(crate) mod state;
pub(crate) mod store;

#[derive(Stage)]
#[stage(name = "ledger", unit = "ValidateHeaderEvent", worker = "Worker")]
pub struct Stage {
    pub upstream: UpstreamPort,
    pub store: PathBuf,
}

impl Stage {
    pub fn new(store: PathBuf) -> Self {
        Self {
            upstream: Default::default(),
            store,
        }
    }
}

pub struct Worker {
    state: state::State<'static, rocksdb::Error>,
}

#[async_trait::async_trait(?Send)]
impl gasket::framework::Worker<Stage> for Worker {
    async fn bootstrap(stage: &Stage) -> Result<Self, WorkerError> {
        let store = store::impl_rocksdb::RocksDB::new(&stage.store).map_err(|e| {
            error!("{e:?}");
            WorkerError::Panic
        })?;

        Ok(Self {
            state: state::State::new(Box::new(store) as Box<dyn Store<Error = rocksdb::Error>>),
        })
    }

    async fn schedule(
        &mut self,
        stage: &mut Stage,
    ) -> Result<WorkSchedule<ValidateHeaderEvent>, WorkerError> {
        let unit = stage.upstream.recv().await.or_panic()?;
        Ok(WorkSchedule::Unit(unit.payload))
    }

    async fn execute(
        &mut self,
        unit: &ValidateHeaderEvent,
        _stage: &mut Stage,
    ) -> Result<(), WorkerError> {
        match unit {
            ValidateHeaderEvent::Validated(_point, raw_block) => {
                let (block_header_hash, block) = parse_block(&raw_block[..]);

                let block_number = block.header.header_body.block_number;
                info!(
                    "applying block={:?} ({:?})",
                    block_number,
                    hex::encode(block_header_hash)
                );

                self.state.forward(block).map_err(|e| {
                    error!("failed to apply block: {e:?}");
                    WorkerError::Panic
                })?;

                info!("applied block={:?}", block_number);
            }

            ValidateHeaderEvent::Rollback(point) => {
                info!("rolling back to {:?}", point);

                if let Err(e) = self.state.backward(point) {
                    match e {
                        BackwardErr::UnknownRollbackPoint(point) => {
                            warn!("tried to roll back to an unknown point: {point:?}")
                        }
                    }
                }

                info!("rolled back to {:?}", point);
            }
        }

        Ok(())
    }
}

fn parse_block(bytes: &[u8]) -> (Hash<32>, MintedBlock<'_>) {
    let (_, block): (u16, MintedBlock<'_>) = cbor::decode(bytes)
        .unwrap_or_else(|_| panic!("failed to decode Conway block: {:?}", hex::encode(bytes)));

    (Hasher::<256>::hash(block.header.raw_cbor()), block)
}
