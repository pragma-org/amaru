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

use amaru_consensus::consensus::{receive_header, ChainSyncEvent, PullEvent};
use gasket::framework::*;
use tracing::{instrument, Level};

pub type UpstreamPort = gasket::messaging::InputPort<ChainSyncEvent>;
pub type DownstreamPort = gasket::messaging::OutputPort<PullEvent>;

#[derive(Default, Stage)]
#[stage(
    name = "consensus.receive_header",
    unit = "ChainSyncEvent",
    worker = "Worker"
)]
pub struct ReceiveHeaderStage {
    pub upstream: UpstreamPort,
    pub downstream: DownstreamPort,
}

impl ReceiveHeaderStage {
    async fn handle_event(&mut self, unit: &ChainSyncEvent) -> Result<(), WorkerError> {
        let event = receive_header::handle_chain_sync(unit).map_err(|_| WorkerError::Recv)?;

        self.downstream.send(event.into()).await.or_panic()?;

        Ok(())
    }
}

pub struct Worker {}

#[async_trait::async_trait(?Send)]
impl gasket::framework::Worker<ReceiveHeaderStage> for Worker {
    async fn bootstrap(_stage: &ReceiveHeaderStage) -> Result<Self, WorkerError> {
        Ok(Self {})
    }

    async fn schedule(
        &mut self,
        stage: &mut ReceiveHeaderStage,
    ) -> Result<WorkSchedule<ChainSyncEvent>, WorkerError> {
        let unit = stage.upstream.recv().await.or_panic()?;

        Ok(WorkSchedule::Unit(unit.payload))
    }

    #[instrument(
        level = Level::TRACE,
        skip_all,
        name = "stage.receive_header"
    )]
    async fn execute(
        &mut self,
        unit: &ChainSyncEvent,
        stage: &mut ReceiveHeaderStage,
    ) -> Result<(), WorkerError> {
        stage.handle_event(unit).await
    }
}
