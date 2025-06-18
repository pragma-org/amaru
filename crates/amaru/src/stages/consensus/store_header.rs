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
use amaru_consensus::consensus::{store_header::StoreHeader, DecodedChainSyncEvent};
use gasket::framework::*;
use tracing::{error, instrument, Level};

pub type UpstreamPort = gasket::messaging::InputPort<DecodedChainSyncEvent>;
pub type DownstreamPort = gasket::messaging::OutputPort<DecodedChainSyncEvent>;

#[derive(Stage)]
#[stage(
    name = "stage.store_header",
    unit = "DecodedChainSyncEvent",
    worker = "Worker"
)]
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

    async fn handle_event(&mut self, event: DecodedChainSyncEvent) -> Result<(), WorkerError> {
        let event = self.store_header.handle_event(event).await.map_err(|e| {
            error!("fail to store header {}", e);
            WorkerError::Recv
        })?;

        send!(&mut self.downstream, event)
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
    ) -> Result<WorkSchedule<DecodedChainSyncEvent>, WorkerError> {
        schedule!(&mut stage.upstream)
    }

    #[instrument(
        level = Level::TRACE,
        skip_all,
        name = "stage.store_header",
    )]
    async fn execute(
        &mut self,
        unit: &DecodedChainSyncEvent,
        stage: &mut StoreHeaderStage,
    ) -> Result<(), WorkerError> {
        adopt_current_span(unit);
        stage.handle_event(unit.clone()).await
    }
}
