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

use amaru_consensus::consensus::{validate_header::ValidateHeader, DecodedChainSyncEvent};
use amaru_kernel::protocol_parameters::GlobalParameters;
use gasket::framework::*;
use tracing::{error, instrument, Level};

use crate::{schedule, send, stages::common::adopt_current_span};

pub type UpstreamPort = gasket::messaging::InputPort<DecodedChainSyncEvent>;
pub type DownstreamPort = gasket::messaging::OutputPort<DecodedChainSyncEvent>;

#[derive(Stage)]
#[stage(
    name = "stage.validate_header",
    unit = "DecodedChainSyncEvent",
    worker = "Worker"
)]
pub struct ValidateHeaderStage {
    pub consensus: ValidateHeader,
    pub upstream: UpstreamPort,
    pub downstream: DownstreamPort,
    pub global_parameters: GlobalParameters,
}

impl ValidateHeaderStage {
    pub fn new(consensus: ValidateHeader, global_parameters: &GlobalParameters) -> Self {
        Self {
            consensus,
            upstream: Default::default(),
            downstream: Default::default(),
            global_parameters: global_parameters.clone(),
        }
    }

    async fn handle_event(&mut self, unit: DecodedChainSyncEvent) -> Result<(), WorkerError> {
        let event = self
            .consensus
            .handle_chain_sync(unit, &self.global_parameters)
            .await
            .map_err(|e| {
                error!("failed to validate header {}", e);
                WorkerError::Recv
            })?;

        send!(&mut self.downstream, event)
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
    ) -> Result<WorkSchedule<DecodedChainSyncEvent>, WorkerError> {
        schedule!(&mut stage.upstream)
    }

    #[instrument(
        level = Level::TRACE,
        skip_all,
        name = "stage.validate_header",
    )]
    async fn execute(
        &mut self,
        unit: &DecodedChainSyncEvent,
        stage: &mut ValidateHeaderStage,
    ) -> Result<(), WorkerError> {
        adopt_current_span(unit);
        stage.handle_event(unit.clone()).await
    }
}
