use crate::{consensus::header_validation::assert_header, sync::PullEvent};
use gasket::framework::*;
use miette::miette;
use ouroboros::ledger::LedgerState;
use pallas_crypto::hash::Hash;
use pallas_network::facades::PeerClient;
use pallas_primitives::conway::Epoch;
use pallas_traverse::MultiEraHeader;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

pub type UpstreamPort = gasket::messaging::InputPort<PullEvent>;
pub type DownstreamPort = gasket::messaging::OutputPort<ValidateHeaderEvent>;
pub type RawBlock = Vec<u8>;
pub type Point = pallas_network::miniprotocols::Point;

pub mod header_validation;
pub mod nonce;

#[derive(Clone)]
pub enum ValidateHeaderEvent {
    Validated(Point, RawBlock),
    Rollback(Point),
}

#[derive(Stage)]
#[stage(name = "header_validation", unit = "PullEvent", worker = "Worker")]
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
                let header = MultiEraHeader::decode(6, None, raw_header)
                    .map_err(|e| miette!(e))
                    .or_panic()?;

                let ledger = stage.ledger.lock().await;
                assert_header(&header, &stage.epoch_to_nonce, &*ledger)?;

                let block = {
                    let mut peer_session = stage.peer_session.lock().await;
                    let client = (*peer_session).blockfetch();
                    let block = client.fetch_single(point.clone()).await.or_restart()?;
                    block
                };

                stage
                    .downstream
                    .send(ValidateHeaderEvent::Validated(point.clone(), block).into())
                    .await
                    .or_panic()?;

                stage.block_count.inc(1);
                stage.track_validation_tip(point);
            }
            PullEvent::Rollback(rollback) => {
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
