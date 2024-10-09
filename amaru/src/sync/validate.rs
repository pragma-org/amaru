use gasket::framework::*;
use miette::miette;
use pallas_network::facades::PeerClient;
use pallas_network::miniprotocols::chainsync::{
    HeaderContent, NextResponse, RollbackBuffer, RollbackEffect, Tip,
};
use pallas_network::miniprotocols::Point;
use pallas_traverse::{MultiEraBlock, MultiEraHeader};
use tracing::{debug, info};

use super::{BlockCbor, PullEvent};

pub type UpstreamPort = gasket::messaging::InputPort<PullEvent>;

#[derive(Stage)]
#[stage(name = "validate", unit = "PullEvent", worker = "Worker")]
pub struct Stage {
    pub upstream: UpstreamPort,

    #[metric]
    block_count: gasket::metrics::Counter,
}

impl Stage {
    pub fn new() -> Self {
        Self {
            upstream: Default::default(),
            block_count: Default::default(),
        }
    }
}

pub struct Worker {
    // TODO: put here any state you need
}

#[async_trait::async_trait(?Send)]
impl gasket::framework::Worker<Stage> for Worker {
    async fn bootstrap(stage: &Stage) -> Result<Self, WorkerError> {
        // TODO: put here any initialization logic you need
        let worker = Self {};

        Ok(worker)
    }

    async fn schedule(
        &mut self,
        stage: &mut Stage,
    ) -> Result<WorkSchedule<PullEvent>, WorkerError> {
        let unit = stage.upstream.recv().await.or_panic()?;

        Ok(WorkSchedule::Unit(unit.payload))
    }

    async fn execute(&mut self, unit: &PullEvent, stage: &mut Stage) -> Result<(), WorkerError> {
        // TODO: do the actual validation

        match unit {
            PullEvent::RollForward(point, _block) => {
                info!(?point, "validating roll forward");
            }
            PullEvent::Rollback(rollback) => {
                info!(?rollback, "validating roll back");
            }
        }

        Ok(())
    }
}
