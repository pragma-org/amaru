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
use amaru_kernel::block::BlockValidationResult;
use gasket::framework::*;

pub type UpstreamPort = gasket::messaging::InputPort<BlockValidationResult>;
pub type DownstreamPort = gasket::messaging::OutputPort<BlockValidationResult>;

#[derive(Stage)]
#[stage(
    name = "consensus.store_block",
    unit = "BlockValidationResult",
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

    async fn handle_event(&mut self, event: BlockValidationResult) -> Result<(), WorkerError> {
        let event = self
            .store_block
            .handle_event(event)
            .await
            .map_err(|_| WorkerError::Recv)?;

        self.downstream.send(event.into()).await.or_panic()?;

        Ok(())
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
    ) -> Result<WorkSchedule<BlockValidationResult>, WorkerError> {
        let unit = stage.upstream.recv().await.or_panic()?;

        Ok(WorkSchedule::Unit(unit.payload))
    }

    async fn execute(
        &mut self,
        unit: &BlockValidationResult,
        stage: &mut StoreBlockStage,
    ) -> Result<(), WorkerError> {
        stage.handle_event(unit.clone()).await
    }
}
