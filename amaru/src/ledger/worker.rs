use crate::consensus::ValidateHeaderEvent;
use crate::ledger::LedgerState;
use gasket::framework::*;
use pallas_codec::minicbor as cbor;
use pallas_crypto::hash::{Hash, Hasher};
use pallas_primitives::conway::MintedBlock;
use tracing::{error, info};

pub type UpstreamPort = gasket::messaging::InputPort<ValidateHeaderEvent>;

#[derive(Stage)]
#[stage(name = "ledger", unit = "ValidateHeaderEvent", worker = "Worker")]
pub struct Stage {
    pub upstream: UpstreamPort,
}

impl Stage {
    pub fn new() -> Self {
        Self {
            upstream: Default::default(),
        }
    }
}

pub struct Worker {
    ledger: LedgerState,
}

#[async_trait::async_trait(?Send)]
impl gasket::framework::Worker<Stage> for Worker {
    async fn bootstrap(_stage: &Stage) -> Result<Self, WorkerError> {
        Ok(Self {
            ledger: LedgerState::new(),
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

                self.ledger
                    .forward(block)
                    .unwrap_or_else(|e| error!("failed to apply block: {e:?}"));

                info!("applied block={:?}", block_number);
            }
            ValidateHeaderEvent::Rollback(point) => {
                info!("rolling back to {:?}", point);

                self.ledger
                    .backward(point)
                    .unwrap_or_else(|e| error!("failed to rollback block: {e:?}"));

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
