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

use amaru_consensus::consensus::{store_header::StoreHeader, PullEvent};
use gasket::framework::*;

pub type UpstreamPort = gasket::messaging::InputPort<PullEvent>;
pub type DownstreamPort = gasket::messaging::OutputPort<PullEvent>;

#[derive(Stage)]
#[stage(name = "consensus.store_header", unit = "PullEvent", worker = "Worker")]
pub struct StoreHeaderStage {
    pub store_header: StoreHeader,
    pub upstream: UpstreamPort,
    pub downstream: DownstreamPort,
}

impl StoreHeaderStage {
    pub fn new(store_header: StoreHeader) -> Self {
        Self {
            store_header,
            upstream: Default::default(),
            downstream: Default::default(),
        }
    }

    async fn handle_event(&mut self, event: &PullEvent) -> Result<(), WorkerError> {
        let event = self
            .store_header
            .handle_event(event)
            .await
            .map_err(|_| WorkerError::Recv)?;

        self.downstream.send(event.into()).await.or_panic()?;

        Ok(())
    }
}

pub struct Worker {}

#[async_trait::async_trait(?Send)]
impl gasket::framework::Worker<StoreHeaderStage> for Worker {
    async fn bootstrap(_stage: &StoreHeaderStage) -> Result<Self, WorkerError> {
        Ok(Self {})
    }

    async fn schedule(
        &mut self,
        stage: &mut StoreHeaderStage,
    ) -> Result<WorkSchedule<PullEvent>, WorkerError> {
        let unit = stage.upstream.recv().await.or_panic()?;

        Ok(WorkSchedule::Unit(unit.payload))
    }

    async fn execute(
        &mut self,
        unit: &PullEvent,
        stage: &mut StoreHeaderStage,
    ) -> Result<(), WorkerError> {
        stage.handle_event(unit).await
    }
}
