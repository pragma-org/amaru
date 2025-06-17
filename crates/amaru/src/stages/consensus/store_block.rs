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

use amaru_consensus::consensus::store_block::StoreBlock;
use amaru_kernel::block::ValidateBlockEvent;
use gasket::framework::*;
use tracing::{instrument, Level};

use crate::{schedule, send, stages::common::adopt_current_span};

pub type UpstreamPort = gasket::messaging::InputPort<ValidateBlockEvent>;
pub type DownstreamPort = gasket::messaging::OutputPort<ValidateBlockEvent>;

#[derive(Stage)]
#[stage(
    name = "stage.store_block",
    unit = "ValidateBlockEvent",
    worker = "Worker"
)]
pub struct StoreBlockStage {
    pub store_block: StoreBlock,
    pub upstream: UpstreamPort,
    pub downstream: DownstreamPort,
}

impl StoreBlockStage {
    pub fn new(store_block: StoreBlock) -> Self {
        Self {
            store_block,
            upstream: Default::default(),
            downstream: Default::default(),
        }
    }

    async fn handle_event(&mut self, event: ValidateBlockEvent) -> Result<(), WorkerError> {
        let event = self.store_block.handle_event(&event).await.map_err(|e| {
            tracing::error!(?e, "Failed to handle store block event");
            WorkerError::Recv
        })?;

        send!(&mut self.downstream, event)
    }
}

pub struct Worker {}

#[async_trait::async_trait(?Send)]
impl gasket::framework::Worker<StoreBlockStage> for Worker {
    async fn bootstrap(_stage: &StoreBlockStage) -> Result<Self, WorkerError> {
        Ok(Self {})
    }

    async fn schedule(
        &mut self,
        stage: &mut StoreBlockStage,
    ) -> Result<WorkSchedule<ValidateBlockEvent>, WorkerError> {
        schedule!(&mut stage.upstream)
    }

    #[instrument(
        level = Level::TRACE,
        skip_all,
        name = "stage.store_block",
    )]
    async fn execute(
        &mut self,
        unit: &ValidateBlockEvent,
        stage: &mut StoreBlockStage,
    ) -> Result<(), WorkerError> {
        adopt_current_span(unit);
        stage.handle_event(unit.clone()).await
    }
}
