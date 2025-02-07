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

use amaru_ouroboros::protocol::{peer::PeerSession, PullEvent, RawHeader};
use gasket::framework::*;
use miette::miette;
use pallas_network::miniprotocols::{
    chainsync::{HeaderContent, NextResponse, Tip},
    Point,
};
use pallas_traverse::MultiEraHeader;
use std::time::Duration;
use tokio::time::timeout;
use tracing::{instrument, trace_span, Level};

pub fn to_traverse(header: &HeaderContent) -> Result<MultiEraHeader<'_>, WorkerError> {
    let out = match header.byron_prefix {
        Some((subtag, _)) => MultiEraHeader::decode(header.variant, Some(subtag), &header.cbor),
        None => MultiEraHeader::decode(header.variant, None, &header.cbor),
    };

    out.or_panic()
}

pub type DownstreamPort = gasket::messaging::OutputPort<PullEvent>;

const EVENT_TARGET: &str = "amaru::sync";

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
        skip(self),
        fields(
            peer = self.peer_session.peer.name,
            intersection.slot = self.intersection.last().unwrap().slot_or_default(),
        ),
    )]
    pub async fn find_intersection(&self) -> Result<(), WorkerError> {
        let mut peer_client = self.peer_session.peer_client.lock().await;
        let client = (*peer_client).chainsync();

        let (point, _) = client
            .find_intersect(self.intersection.clone())
            .await
            .or_restart()?;

        let _intersection = point.ok_or(miette!("couldn't find intersect")).or_panic()?;
        Ok(())
    }

    pub async fn roll_forward(&mut self, header: &HeaderContent) -> Result<(), WorkerError> {
        let span_forward = trace_span!(
            target: EVENT_TARGET,
            "pull.roll_forward",
            header.slot = tracing::field::Empty,
            header.hash = tracing::field::Empty,
            peer = self.peer_session.peer.name
        );

        let peer = &self.peer_session.peer;
        let header = to_traverse(header).or_panic()?;
        let point = Point::Specific(header.slot(), header.hash().to_vec());

        let raw_header: RawHeader = header.cbor().to_vec();

        self.downstream
            .send(PullEvent::RollForward(peer.clone(), point, raw_header, span_forward).into())
            .await
            .or_panic()
    }

    pub async fn roll_back(&mut self, point: Point, tip: Tip) -> Result<(), WorkerError> {
        let span_backward = trace_span!(
            target: EVENT_TARGET,
            "pull.roll_back",
            point = tracing::field::Empty,
            tip = tracing::field::Empty,
            peer = self.peer_session.peer.name
        );

        self.track_tip(&tip);

        let peer = &self.peer_session.peer;
        self.downstream
            .send(PullEvent::Rollback(peer.clone(), point, span_backward).into())
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
                stage.roll_back(point, tip).await?;
            }
            NextResponse::Await => {}
        };

        Ok(())
    }
}
