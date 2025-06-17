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

use crate::{schedule, send, stages::common::adopt_current_span};
use amaru_consensus::consensus::{receive_header, ChainSyncEvent, DecodedChainSyncEvent};
use gasket::framework::*;
use tracing::{error, instrument, Level};

pub type UpstreamPort = gasket::messaging::InputPort<ChainSyncEvent>;
pub type DownstreamPort = gasket::messaging::OutputPort<DecodedChainSyncEvent>;

#[derive(Default, Stage)]
#[stage(
    name = "stage.receive_header",
    unit = "ChainSyncEvent",
    worker = "Worker"
)]
pub struct ReceiveHeaderStage {
    pub upstream: UpstreamPort,
    pub downstream: DownstreamPort,
}

impl ReceiveHeaderStage {
    async fn handle_event(&mut self, sync_event: ChainSyncEvent) -> Result<(), WorkerError> {
        let event = receive_header::handle_chain_sync(sync_event).map_err(|e| {
            error!("fail to handle chain sync {}", e);
            WorkerError::Recv
        })?;

        send!(&mut self.downstream, event)
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
        schedule!(&mut stage.upstream)
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
        adopt_current_span(unit);
        stage.handle_event(unit.clone()).await
    }
}
