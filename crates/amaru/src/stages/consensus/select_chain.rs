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

use amaru_consensus::consensus::{select_chain::SelectChain, PullEvent, ValidateHeaderEvent};
use gasket::framework::*;

pub type UpstreamPort = gasket::messaging::InputPort<PullEvent>;
pub type DownstreamPort = gasket::messaging::OutputPort<ValidateHeaderEvent>;

#[derive(Stage)]
#[stage(name = "consensus.select_chain", unit = "PullEvent", worker = "Worker")]
pub struct SelectChainStage {
    pub select_chain: SelectChain,
    pub upstream: UpstreamPort,
    pub downstream: DownstreamPort,
}

impl SelectChainStage {
    pub fn new(select_chain: SelectChain) -> Self {
        Self {
            select_chain,
            upstream: Default::default(),
            downstream: Default::default(),
        }
    }

    async fn handle_event(&mut self, pull: &PullEvent) -> Result<(), WorkerError> {
        let events = self.select_chain.handle_chain_sync(pull).await.or_panic()?;

        for event in events {
            self.downstream.send(event.into()).await.or_panic()?;
        }

        Ok(())
    }
}

pub struct Worker {}

#[async_trait::async_trait(?Send)]
impl gasket::framework::Worker<SelectChainStage> for Worker {
    async fn bootstrap(_stage: &SelectChainStage) -> Result<Self, WorkerError> {
        Ok(Self {})
    }

    async fn schedule(
        &mut self,
        stage: &mut SelectChainStage,
    ) -> Result<WorkSchedule<PullEvent>, WorkerError> {
        let unit = stage.upstream.recv().await.or_panic()?;

        Ok(WorkSchedule::Unit(unit.payload))
    }

    async fn execute(
        &mut self,
        unit: &PullEvent,
        stage: &mut SelectChainStage,
    ) -> Result<(), WorkerError> {
        stage.handle_event(unit).await
    }
}
