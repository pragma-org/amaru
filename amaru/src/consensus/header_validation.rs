use crate::{
    consensus::{Point, ValidateHeaderEvent},
    sync::PullEvent,
};
use gasket::framework::*;
use miette::miette;
use ouroboros::{ledger::LedgerState, validator::Validator};
use ouroboros_praos::consensus::BlockValidator;
use pallas_crypto::hash::Hash;
use pallas_math::math::{FixedDecimal, FixedPrecision};
use pallas_network::facades::PeerClient;
use pallas_primitives::conway::Epoch;
use pallas_traverse::MultiEraHeader;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tracing::{info, trace};

pub type UpstreamPort = gasket::messaging::InputPort<PullEvent>;
pub type DownstreamPort = gasket::messaging::OutputPort<ValidateHeaderEvent>;

/// Mocking this calculation specifically for preprod. We need to know the epoch for the slot so
/// we can look up the epoch nonce.
fn epoch_for_preprod_slot(slot: u64) -> u64 {
    let shelley_epoch_length = 432000; // 5 days in seconds
    let shelley_transition_epoch: u64 = 4;
    let byron_protocol_consts_k: u64 = 2160;
    let byron_epoch_length = 10 * byron_protocol_consts_k;
    let byron_slots = byron_epoch_length * shelley_transition_epoch;
    let shelley_slots = slot - byron_slots;
    let epoch = (shelley_slots / shelley_epoch_length) + shelley_transition_epoch;

    epoch
}

#[derive(Stage)]
#[stage(name = "validate", unit = "PullEvent", worker = "Worker")]
pub struct Stage {
    peer_session: Arc<Mutex<PeerClient>>,

    ledger: Arc<Mutex<dyn LedgerState>>,
    epoch_to_nonce: HashMap<Epoch, Hash<32>>,

    pub upstream: UpstreamPort,
    pub downstream: DownstreamPort,

    #[metric]
    block_count: gasket::metrics::Counter,

    #[metric]
    rollback_count: gasket::metrics::Counter,

    #[metric]
    validation_tip: gasket::metrics::Gauge,
}

impl Stage {
    pub fn new(
        peer_session: Arc<Mutex<PeerClient>>,
        ledger: Arc<Mutex<dyn LedgerState>>,
        epoch_to_nonce: HashMap<Epoch, Hash<32>>,
    ) -> Self {
        Self {
            peer_session,
            ledger,
            epoch_to_nonce,
            upstream: Default::default(),
            downstream: Default::default(),
            block_count: Default::default(),
            rollback_count: Default::default(),
            validation_tip: Default::default(),
        }
    }

    fn track_validation_tip(&self, tip: &Point) {
        self.validation_tip.set(tip.slot_or_default() as i64);
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
    ) -> Result<WorkSchedule<PullEvent>, WorkerError> {
        let unit = stage.upstream.recv().await.or_panic()?;

        Ok(WorkSchedule::Unit(unit.payload))
    }

    async fn execute(&mut self, unit: &PullEvent, stage: &mut Stage) -> Result<(), WorkerError> {
        match unit {
            PullEvent::RollForward(point, raw_header) => {
                info!(?point, "validating roll forward");
                let header = MultiEraHeader::decode(6, None, raw_header)
                    .map_err(|e| miette!(e))
                    .or_panic()?;

                match header {
                    MultiEraHeader::BabbageCompatible(_) => {
                        let minted_header = header.as_babbage().unwrap();
                        let epoch = epoch_for_preprod_slot(minted_header.header_body.slot);

                        // TODO: This is awkward, and should probably belong to the LedgerState
                        // abstraction? The ledger shall keep track of the rolling nonce and
                        // provide some endpoint for the consensus to access it.
                        let epoch_nonce = stage
                            .epoch_to_nonce
                            .get(&epoch)
                            .ok_or(miette!("epoch nonce not found"))
                            .or_panic()?;

                        let active_slots_coeff: FixedDecimal =
                            FixedDecimal::from(5u64) / FixedDecimal::from(100u64);
                        let c = (FixedDecimal::from(1u64) - active_slots_coeff).ln();
                        let ledger = stage.ledger.lock().unwrap();
                        let block_validator =
                            BlockValidator::new(minted_header, &*ledger, epoch_nonce, &c);
                        block_validator.validate().or_panic()?;
                        info!(?minted_header.header_body.block_number, "validated block");
                    }
                    MultiEraHeader::ShelleyCompatible(_) => {
                        trace!("shelley compatible header, skipping validation");
                    }
                    MultiEraHeader::EpochBoundary(_) => {
                        trace!("epoch boundary header, skipping validation");
                    }
                    MultiEraHeader::Byron(_) => {
                        trace!("byron header, skipping validation");
                    }
                }

                let block = {
                    info!("fetching block...");
                    let mut peer_session = stage.peer_session.lock().unwrap();
                    let client = (*peer_session).blockfetch();
                    let block = client.fetch_single(point.clone()).await.or_restart()?;
                    info!("fetched block done.");
                    block
                };

                // info!(?block, "fetched block");

                info!("sending validated block downstream");
                stage
                    .downstream
                    .send(ValidateHeaderEvent::Validated(point.clone(), block).into())
                    .await
                    .or_panic()?;

                info!("sent validated block downstream");

                stage.block_count.inc(1);
                stage.track_validation_tip(point);
            }
            PullEvent::Rollback(rollback) => {
                info!(?rollback, "validating roll back");

                stage
                    .downstream
                    .send(ValidateHeaderEvent::Rollback(rollback.clone()).into())
                    .await
                    .or_panic()?;

                stage.rollback_count.inc(1);
                stage.track_validation_tip(rollback);
            }
        }

        Ok(())
    }
}
