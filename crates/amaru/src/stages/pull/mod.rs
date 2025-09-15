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

use crate::{
    send,
    stages::{metrics::MetricsEvent, pull::metrics::PullMetrics},
};
use amaru_consensus::consensus::events::ChainSyncEvent;
use amaru_kernel::string_utils::ListToString;
use amaru_kernel::{Point, peer::Peer};
use amaru_network::chain_sync_client::{ChainSyncClientError, RawHeader, to_traverse};
use amaru_network::{chain_sync_client::ChainSyncClient, point::from_network_point};
use gasket::framework::{Stage as StageTrait, *};
use pallas_network::miniprotocols::chainsync::{Client, HeaderContent, NextResponse};
use tracing::{Level, Span, error, instrument};

pub mod metrics;

pub type DownstreamPort = gasket::messaging::OutputPort<ChainSyncEvent>;
pub type MetricsDownstreamPort = gasket::messaging::OutputPort<MetricsEvent>;

pub enum WorkUnit {
    Pull,
    Await,
    Intersect,
}

#[derive(StageTrait)]
#[stage(name = "stage.chain_sync_client", unit = "WorkUnit", worker = "Worker")]
pub struct Stage {
    pub peer: Peer,
    pub client: ChainSyncClient,
    pub downstream: DownstreamPort,
    pub metrics_downstream: MetricsDownstreamPort,
}

impl Stage {
    pub fn new(peer: Peer, chain_sync: Client<HeaderContent>, intersection: Vec<Point>) -> Self {
        let client = ChainSyncClient::new(peer.clone(), chain_sync, intersection);
        Self {
            peer,
            client,
            downstream: Default::default(),
            metrics_downstream: MetricsDownstreamPort::default(),
        }
    }

    pub async fn find_intersection(&mut self) -> Result<Point, ChainSyncClientError> {
        self.client.find_intersection().await
    }

    pub async fn roll_forward(&mut self, header: &HeaderContent) -> Result<(), WorkerError> {
        let peer = &self.peer;
        let header = to_traverse(header).map_err(|_| WorkerError::Panic)?;
        let point = Point::Specific(header.slot(), header.hash().to_vec());
        let raw_header: RawHeader = header.cbor().to_vec();

        let event = ChainSyncEvent::RollForward {
            peer: peer.clone(),
            point,
            raw_header,
            span: Span::current(),
        };
        send!(&mut self.downstream, event)
    }

    pub async fn roll_back(&mut self, rollback_point: Point) -> Result<(), WorkerError> {
        let event = ChainSyncEvent::Rollback {
            peer: self.peer.clone(),
            rollback_point,
            span: Span::current(),
        };
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
            if stage.client.has_agency() {
                // should request next block
                Ok(WorkSchedule::Unit(WorkUnit::Pull))
            } else {
                // should await for next block
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
        let next = match unit {
            WorkUnit::Pull => stage.client.request_next().await.or_panic()?,
            WorkUnit::Await => stage.client.await_next().await.or_panic()?,
            WorkUnit::Intersect => match stage.find_intersection().await {
                Err(err) => {
                    error!("No intersection found with {}: {}", stage.client.peer, err);
                    return Err(WorkerError::Panic);
                }
                Ok(intersection) => {
                    self.initialised = true;
                    error!(
                        "Intersection found with {}: {:?}",
                        stage.client.peer, intersection
                    );
                    return Ok(());
                }
            },
        };

        match next {
            NextResponse::RollForward(header, _tip) => {
                stage.roll_forward(&header).await?;
                PullMetrics::record_header_size_bytes(&mut stage.metrics_downstream, 1).await?;
            }
            NextResponse::RollBackward(point, _tip) => {
                stage.roll_back(from_network_point(&point)).await?;
            }
            NextResponse::Await => stage.caught_up().await?,
        };

        Ok(())
    }
}
