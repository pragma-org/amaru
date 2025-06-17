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

use amaru_consensus::consensus::{
    select_chain::SelectChain, DecodedChainSyncEvent, ValidateHeaderEvent,
};
use gasket::framework::*;
use tracing::{error, instrument, Level};

use crate::{schedule, send, stages::common::adopt_current_span};

pub type UpstreamPort = gasket::messaging::InputPort<DecodedChainSyncEvent>;
pub type DownstreamPort = gasket::messaging::OutputPort<ValidateHeaderEvent>;

#[derive(Stage)]
#[stage(
    name = "stage.select_chain",
    unit = "DecodedChainSyncEvent",
    worker = "Worker"
)]
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

    async fn handle_event(&mut self, sync_event: DecodedChainSyncEvent) -> Result<(), WorkerError> {
        let events = self
            .select_chain
            .handle_chain_sync(sync_event)
            .await
            .map_err(|e| {
                error!(error=%e, "fail to select chain");
                WorkerError::Recv
            })?;

        for event in events {
            send!(&mut self.downstream, event)?;
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
    ) -> Result<WorkSchedule<DecodedChainSyncEvent>, WorkerError> {
        schedule!(&mut stage.upstream)
    }

    #[instrument(
        level = Level::TRACE,
        skip_all,
        name = "stage.select_chain",
    )]
    async fn execute(
        &mut self,
        unit: &DecodedChainSyncEvent,
        stage: &mut SelectChainStage,
    ) -> Result<(), WorkerError> {
        adopt_current_span(unit);
        stage.handle_event(unit.clone()).await
    }
}
