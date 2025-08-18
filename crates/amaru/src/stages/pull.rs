// Copyright 2024 PRAGMA
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

use crate::send;
use amaru_consensus::consensus::ChainSyncEvent;
use amaru_kernel::Point;
use amaru_network::{
    chain_sync_client::{ChainSyncClient, PullResult},
    point::from_network_point,
    session::PeerSession,
};
use gasket::framework::*;
use pallas_network::miniprotocols::chainsync::{HeaderContent, NextResponse, Tip};
use std::sync::{Arc, RwLock};
use tracing::{Level, instrument};

pub type DownstreamPort = gasket::messaging::OutputPort<ChainSyncEvent>;

pub enum WorkUnit {
    Pull,
    Await,
}

#[derive(Stage)]
#[stage(name = "stage.chain_sync_client", unit = "WorkUnit", worker = "Worker")]
pub struct Stage {
    pub client: ChainSyncClient<HeaderContent>,
    pub downstream: DownstreamPort,
}

impl Stage {
    pub fn new(
        peer_session: PeerSession,
        intersection: Vec<Point>,
        is_catching_up: Arc<RwLock<bool>>,
    ) -> Self {
        Self {
            client: ChainSyncClient::new(peer_session, intersection, is_catching_up),
            downstream: Default::default(),
        }
    }

    pub async fn find_intersection(&mut self) -> Result<(), WorkerError> {
        self.client.find_intersection().await.or_panic()
    }

    pub async fn roll_forward(&mut self, header: &HeaderContent) -> Result<(), WorkerError> {
        let event = self.client.roll_forward(header).or_panic()?;
        send!(&mut self.downstream, event)
    }

    pub async fn roll_back(&mut self, rollback_point: Point, tip: Tip) -> Result<(), WorkerError> {
        let event = self.client.roll_back(rollback_point, tip).or_panic()?;
        self.downstream.send(event.into()).await.or_panic()
    }
}

pub struct Worker {}

#[async_trait::async_trait(?Send)]
impl gasket::framework::Worker<Stage> for Worker {
    async fn bootstrap(stage: &Stage) -> Result<Self, WorkerError> {
        //stage.find_intersection().await?;

        let worker = Self {};

        Ok(worker)
    }

    async fn schedule(&mut self, stage: &mut Stage) -> Result<WorkSchedule<WorkUnit>, WorkerError> {
        // if stage.client.has_agency().await {
        //     // should request next block
        //     Ok(WorkSchedule::Unit(WorkUnit::Pull))
        // } else {
        //     // should await for next block
        //     Ok(WorkSchedule::Unit(WorkUnit::Await))
        // }
        Ok(WorkSchedule::Unit(WorkUnit::Await))
    }

    #[instrument(
        level = Level::TRACE,
        name = "stage.pull",
        skip_all,
    )]
    async fn execute(&mut self, unit: &WorkUnit, stage: &mut Stage) -> Result<(), WorkerError> {
        let next = match unit {
            WorkUnit::Pull => {
                let result = stage.client.pull_batch().await.or_panic()?;
                match result {
                    PullResult::ForwardBatch(_) => todo!(),
                    PullResult::RollBack(_) => todo!(),
                    PullResult::Nothing => todo!(),
                }
            }
            WorkUnit::Await => todo!(), // stage.client.await_next().await.or_panic()?,
        };

        match next {
            NextResponse::RollForward(header, _tip) => {
                stage.roll_forward(&header).await?;
            }
            NextResponse::RollBackward(point, tip) => {
                stage.roll_back(from_network_point(&point), tip).await?;
            }
            NextResponse::Await => {}
        };

        Ok(())
    }
}
