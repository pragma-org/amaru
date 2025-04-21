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

use amaru_consensus::consensus::{validate_header::ValidateHeader, PullEvent};
use gasket::framework::*;

pub type UpstreamPort = gasket::messaging::InputPort<PullEvent>;
pub type DownstreamPort = gasket::messaging::OutputPort<PullEvent>;

#[derive(Stage)]
#[stage(
    name = "consensus.validate_header",
    unit = "PullEvent",
    worker = "Worker"
)]
pub struct ValidateHeaderStage {
    pub consensus: ValidateHeader,
    pub upstream: UpstreamPort,
    pub downstream: DownstreamPort,
}

impl ValidateHeaderStage {
    pub fn new(consensus: ValidateHeader) -> Self {
        Self {
            consensus,
            upstream: Default::default(),
            downstream: Default::default(),
        }
    }

    async fn handle_event(&mut self, unit: &PullEvent) -> Result<(), WorkerError> {
        let event = self.consensus.handle_chain_sync(unit).await.or_panic()?;

        self.downstream.send(event.into()).await.or_panic()?;

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
