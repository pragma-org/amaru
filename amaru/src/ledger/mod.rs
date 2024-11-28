use crate::{
    consensus::ValidateHeaderEvent,
    ledger::{
        kernel::{Hash, Hasher, MintedBlock, Point},
        state::BackwardErr,
    },
};
use gasket::framework::*;
use pallas_codec::minicbor as cbor;
use std::{path::Path, sync::Arc};
use tokio::sync::Mutex;
use tracing::{error, info_span, warn};

pub type UpstreamPort = gasket::messaging::InputPort<ValidateHeaderEvent>;

pub mod kernel;
pub mod state;
pub mod store;

#[derive(Stage)]
#[stage(name = "ledger", unit = "ValidateHeaderEvent", worker = "Worker")]
pub struct Stage {
    pub upstream: UpstreamPort,
    pub state: Arc<Mutex<state::State<'static, rocksdb::Error>>>,
}

impl Stage {
    pub fn new(store: &Path) -> Self {
        let store = store::impl_rocksdb::RocksDB::new(store)
            .unwrap_or_else(|e| panic!("unable to open ledger store: {e:?}"));

        Self {
            upstream: Default::default(),
            state: Arc::new(Mutex::new(state::State::new(Arc::new(
                std::sync::Mutex::new(store),
            )))),
        }
    }
}

pub struct Worker {}

#[async_trait::async_trait(?Send)]
impl gasket::framework::Worker<Stage> for Worker {
    async fn bootstrap(_stage: &Stage) -> Result<Self, WorkerError> {
        Ok(Self {})
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
        stage: &mut Stage,
    ) -> Result<(), WorkerError> {
        match unit {
            ValidateHeaderEvent::Validated(_point, raw_block) => {
                let (block_header_hash, block) = parse_block(&raw_block[..]);

                let span_forward = info_span!(
                    "applying_block",
                    height = block.header.header_body.block_number,
                    slot = block.header.header_body.slot,
                    header_hash = hex::encode(block_header_hash)
                )
                .entered();

                let mut state = stage.state.lock().await;

                state.forward(block).map_err(|e| {
                    error!(error = ?e, "failed to forward block");
                    WorkerError::Panic
                })?;

                span_forward.exit();
            }

            ValidateHeaderEvent::Rollback(point) => {
                let span_backward = info_span!(
                    "rolling_back",
                    slot = point.slot_or_default(),
                    header_hash = if let Point::Specific(_, header_hash) = point {
                        hex::encode(header_hash)
                    } else {
                        String::new()
                    }
                )
                .entered();

                let mut state = stage.state.lock().await;

                if let Err(e) = state.backward(point) {
                    match e {
                        BackwardErr::UnknownRollbackPoint(_) => {
                            warn!("tried to roll back to an unknown point")
                        }
                    }
                }

                span_backward.exit();
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
