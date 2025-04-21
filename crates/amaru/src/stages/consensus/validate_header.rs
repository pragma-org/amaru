// Copyright 2025 PRAGMA
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use amaru_consensus::{
    consensus::{header_validation::Consensus, ValidateHeaderEvent},
    peer::Peer,
};
use amaru_kernel::Point;
use gasket::framework::*;
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;

#[derive(Clone, Debug)]
pub enum PullEvent {
    RollForward(Peer, Point, Vec<u8>, Span),
    Rollback(Peer, Point),
}

pub type UpstreamPort = gasket::messaging::InputPort<PullEvent>;
pub type DownstreamPort = gasket::messaging::OutputPort<ValidateHeaderEvent>;

#[derive(Stage)]
#[stage(name = "consensus.header", unit = "PullEvent", worker = "Worker")]
pub struct ValidateHeaderStage {
    pub consensus: Consensus,
    pub upstream: UpstreamPort,
    pub downstream: DownstreamPort,
}

impl ValidateHeaderStage {
    pub fn new(consensus: Consensus) -> Self {
        Self {
            consensus,
            upstream: Default::default(),
            downstream: Default::default(),
        }
    }

    async fn handle_event(&mut self, unit: &PullEvent) -> Result<(), WorkerError> {
        let events = match unit {
            PullEvent::RollForward(peer, point, raw_header, span) => {
                // Restore parent span
                Span::current().set_parent(span.context());
                self.consensus
                    .handle_roll_forward(peer, point, raw_header)
                    .await
                    .or_panic()?
            }
            PullEvent::Rollback(peer, rollback) => self
                .consensus
                .handle_roll_back(peer, rollback)
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
impl gasket::framework::Worker<ValidateHeaderStage> for Worker {
    async fn bootstrap(_stage: &ValidateHeaderStage) -> Result<Self, WorkerError> {
        Ok(Self {})
    }

    async fn schedule(
        &mut self,
        stage: &mut ValidateHeaderStage,
    ) -> Result<WorkSchedule<PullEvent>, WorkerError> {
        let unit = stage.upstream.recv().await.or_panic()?;

        Ok(WorkSchedule::Unit(unit.payload))
    }

    async fn execute(
        &mut self,
        unit: &PullEvent,
        stage: &mut ValidateHeaderStage,
    ) -> Result<(), WorkerError> {
        stage.handle_event(unit).await
    }
}
