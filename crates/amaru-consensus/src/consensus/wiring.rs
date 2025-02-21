use crate::{peer::Peer, Point};
use amaru_ledger::ValidateBlockEvent;
use gasket::framework::*;
use tracing::Span;

use super::header_validation::Consensus;

#[derive(Clone)]
pub enum PullEvent {
    RollForward(Peer, Point, Vec<u8>, Span),
    Rollback(Peer, Point, Span),
}

pub type UpstreamPort = gasket::messaging::InputPort<PullEvent>;
pub type DownstreamPort = gasket::messaging::OutputPort<ValidateBlockEvent>;

#[derive(Stage)]
#[stage(name = "consensus.header", unit = "PullEvent", worker = "Worker")]
pub struct HeaderStage {
    pub consensus: Consensus,
    pub upstream: UpstreamPort,
    pub downstream: DownstreamPort,
}

impl HeaderStage {
    pub fn new(consensus: Consensus) -> Self {
        Self {
            consensus,
            upstream: Default::default(),
            downstream: Default::default(),
        }
    }

    async fn handle_event(&mut self, unit: &PullEvent) -> Result<(), WorkerError> {
        let events = match unit {
            PullEvent::RollForward(peer, point, raw_header, span) => self
                .consensus
                .handle_roll_forward(peer, point, raw_header, span)
                .await
                .or_panic()?,
            PullEvent::Rollback(peer, rollback, span) => self
                .consensus
                .handle_roll_back(peer, rollback, span)
                .await
                .or_panic()?,
        };

        for event in events {
            self.downstream.send(event.into()).await.or_panic()?;
        }

        Ok(())
    }
}

pub struct Worker {}

#[async_trait::async_trait(?Send)]
impl gasket::framework::Worker<HeaderStage> for Worker {
    async fn bootstrap(_stage: &HeaderStage) -> Result<Self, WorkerError> {
        Ok(Self {})
    }

    async fn schedule(
        &mut self,
        stage: &mut HeaderStage,
    ) -> Result<WorkSchedule<PullEvent>, WorkerError> {
        let unit = stage.upstream.recv().await.or_panic()?;

        Ok(WorkSchedule::Unit(unit.payload))
    }

    async fn execute(
        &mut self,
        unit: &PullEvent,
        stage: &mut HeaderStage,
    ) -> Result<(), WorkerError> {
        stage.handle_event(unit).await
    }
}
