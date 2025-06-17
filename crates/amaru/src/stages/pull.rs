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

use crate::point::{from_network_point, to_network_point};
use crate::{send, stages::PeerSession};
use amaru_consensus::{consensus::ChainSyncEvent, RawHeader};
use amaru_kernel::Point;
use anyhow::anyhow;
use gasket::framework::*;
use pallas_network::miniprotocols::chainsync::{HeaderContent, NextResponse, Tip};
use pallas_traverse::MultiEraHeader;
use std::time::Duration;
use tokio::time::timeout;
use tracing::{instrument, Level, Span};

pub fn to_traverse(header: &HeaderContent) -> Result<MultiEraHeader<'_>, WorkerError> {
    let out = match header.byron_prefix {
        Some((subtag, _)) => MultiEraHeader::decode(header.variant, Some(subtag), &header.cbor),
        None => MultiEraHeader::decode(header.variant, None, &header.cbor),
    };

    out.or_panic()
}

pub type DownstreamPort = gasket::messaging::OutputPort<ChainSyncEvent>;

pub enum WorkUnit {
    Pull,
    Await,
}

#[derive(Stage)]
#[stage(name = "pull", unit = "WorkUnit", worker = "Worker")]
pub struct Stage {
    pub peer_session: PeerSession,
    intersection: Vec<Point>,

    pub downstream: DownstreamPort,

    #[metric]
    chain_tip: gasket::metrics::Gauge,
}

impl Stage {
    pub fn new(peer_session: PeerSession, intersection: Vec<Point>) -> Self {
        Self {
            peer_session,
            intersection,
            downstream: Default::default(),
            chain_tip: Default::default(),
        }
    }

    fn track_tip(&self, tip: &Tip) {
        self.chain_tip.set(tip.0.slot_or_default() as i64);
    }

    #[instrument(
        level = Level::TRACE,
        skip_all,
        name = "stage.pull.find_intersection",
        fields(
            peer = self.peer_session.peer.name,
            intersection.slot = %self.intersection.last().unwrap().slot_or_default(),
        ),
    )]
    #[allow(clippy::unwrap_used)]
    pub async fn find_intersection(&self) -> Result<(), WorkerError> {
        let mut peer_client = self.peer_session.peer_client.lock().await;
        let client = (*peer_client).chainsync();
        let (point, _) = client
            .find_intersect(
                self.intersection
                    .iter()
                    .cloned()
                    .map(to_network_point)
                    .collect(),
            )
            .await
            .or_restart()?;

        let _intersection = point.ok_or(anyhow!("couldn't find intersect")).or_panic()?;
        Ok(())
    }

    pub async fn roll_forward(&mut self, header: &HeaderContent) -> Result<(), WorkerError> {
        let peer = &self.peer_session.peer;
        let header = to_traverse(header).or_panic()?;
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

    pub async fn roll_back(&mut self, rollback_point: Point, tip: Tip) -> Result<(), WorkerError> {
        self.track_tip(&tip);

        let peer = &self.peer_session.peer;
        self.downstream
            .send(
                ChainSyncEvent::Rollback {
                    peer: peer.clone(),
                    rollback_point,
                    span: Span::current(),
                }
                .into(),
            )
            .await
            .or_panic()
    }
}

pub struct Worker {}

#[async_trait::async_trait(?Send)]
impl gasket::framework::Worker<Stage> for Worker {
    async fn bootstrap(stage: &Stage) -> Result<Self, WorkerError> {
        stage.find_intersection().await?;

        let worker = Self {};

        Ok(worker)
    }

    async fn schedule(&mut self, stage: &mut Stage) -> Result<WorkSchedule<WorkUnit>, WorkerError> {
        let mut peer_client = stage.peer_session.lock().await;
        let client = (*peer_client).chainsync();

        if client.has_agency() {
            // should request next block
            Ok(WorkSchedule::Unit(WorkUnit::Pull))
        } else {
            // should await for next block
            Ok(WorkSchedule::Unit(WorkUnit::Await))
        }
    }

    #[instrument(
        level = Level::TRACE,
        name = "stage.pull",
        skip_all,
    )]
    async fn execute(&mut self, unit: &WorkUnit, stage: &mut Stage) -> Result<(), WorkerError> {
        let next = {
            let mut peer_client = stage.peer_session.lock().await;
            let client = (*peer_client).chainsync();

            match unit {
                WorkUnit::Pull => client.request_next().await.or_restart()?,
                WorkUnit::Await => {
                    //FIXME: This isn't ideal to use a timeout because we won't see the block the second
                    // it arrives. Ideally, we could just recv_while_must_reply().await forever, but that
                    // causes worker starvation downstream because they cannot lock() the peer_session.
                    match timeout(Duration::from_secs(1), client.recv_while_must_reply()).await {
                        Ok(result) => result.or_restart()?,
                        Err(_) => Err(WorkerError::Retry)?,
                    }
                }
            }
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
