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
use amaru_kernel::{Point, peer::Peer};
use amaru_network::{
    chain_sync_client::{ChainSyncClient, PullResult, new_with_session},
    point::from_network_point,
    session::PeerSession,
};
use gasket::framework::*;
use pallas_network::miniprotocols::chainsync::{HeaderContent, NextResponse, Tip};
use tracing::{Level, Span, instrument};

pub type DownstreamPort = gasket::messaging::OutputPort<ChainSyncEvent>;

pub enum WorkUnit {
    Intersect,
    Pull,
    Await,
}

#[derive(Stage)]
#[stage(name = "stage.chain_sync_client", unit = "WorkUnit", worker = "Worker")]
pub struct Stage {
    pub peer: Peer,
    pub client: ChainSyncClient<HeaderContent>,
    pub downstream: DownstreamPort,
}

impl Stage {
    pub fn new(peer_session: PeerSession, intersection: Vec<Point>) -> Self {
        Self {
            peer: peer_session.peer.clone(),
            client: new_with_session(peer_session, &intersection),
            downstream: Default::default(),
        }
    }

    pub async fn find_intersection(&mut self) -> Result<(), WorkerError> {
        self.client.find_intersection().await.or_panic()
    }

    pub async fn roll_forward(&mut self, headers: &Vec<HeaderContent>) -> Result<(), WorkerError> {
        for header in headers {
            let event = self.client.roll_forward(header).or_panic()?;
            send!(&mut self.downstream, event)?;
        }
        Ok(())
    }

    pub async fn roll_back(&mut self, rollback_point: Point, tip: Tip) -> Result<(), WorkerError> {
        let event = self.client.roll_back(rollback_point, tip).or_panic()?;
        self.downstream.send(event.into()).await.or_panic()
    }

    pub async fn caught_up(&mut self) -> Result<(), WorkerError> {
        let event = ChainSyncEvent::CaughtUp {
            peer: self.peer.clone(),
            span: Span::current(),
        };
        self.downstream.send(event.into()).await.or_panic()
    }
}

pub struct Worker {
    initialised: bool,
}

#[async_trait::async_trait(?Send)]
impl gasket::framework::Worker<Stage> for Worker {
    async fn bootstrap(_stage: &Stage) -> Result<Self, WorkerError> {
        let worker = Self { initialised: false };

        Ok(worker)
    }

    async fn schedule(&mut self, stage: &mut Stage) -> Result<WorkSchedule<WorkUnit>, WorkerError> {
        if self.initialised {
            if stage.client.has_agency().await {
                Ok(WorkSchedule::Unit(WorkUnit::Pull))
            } else {
                Ok(WorkSchedule::Unit(WorkUnit::Await))
            }
        } else {
            Ok(WorkSchedule::Unit(WorkUnit::Intersect))
        }
    }

    #[instrument(
        level = Level::TRACE,
        name = "stage.pull",
        skip_all,
    )]
    async fn execute(&mut self, unit: &WorkUnit, stage: &mut Stage) -> Result<(), WorkerError> {
        match unit {
            WorkUnit::Intersect => {
                stage.find_intersection().await?;
                self.initialised = true;
            }
            WorkUnit::Pull => {
                let result = stage.client.pull_batch().await.or_restart()?;
                match result {
                    PullResult::ForwardBatch(header_contents) => {
                        stage.roll_forward(&header_contents).await?
                    }
                    PullResult::RollBack(point, tip) => stage.roll_back(point, tip).await?,
                    PullResult::Nothing => stage.caught_up().await?,
                }
            }
            WorkUnit::Await => match stage.client.await_next().await.or_restart()? {
                NextResponse::RollForward(header_content, _tip) => {
                    stage.roll_forward(&vec![header_content]).await?
                }
                NextResponse::RollBackward(point, tip) => {
                    stage.roll_back(from_network_point(&point), tip).await?
                }
                NextResponse::Await => (),
            },
        };

        Ok(())
    }
}
