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

use gasket::framework::*;
use miette::miette;
use pallas_network::{
    facades::PeerClient,
    miniprotocols::{
        chainsync::{HeaderContent, NextResponse, Tip},
        Point,
    },
};
use pallas_traverse::MultiEraHeader;
use std::{sync::Arc, time::Duration};
use tokio::{sync::Mutex, time::timeout};
use tracing::info_span;

use super::{PullEvent, RawHeader};

const EVENT_TARGET: &str = "amaru::sync";

fn to_traverse(header: &HeaderContent) -> Result<MultiEraHeader<'_>, WorkerError> {
    let out = match header.byron_prefix {
        Some((subtag, _)) => MultiEraHeader::decode(header.variant, Some(subtag), &header.cbor),
        None => MultiEraHeader::decode(header.variant, None, &header.cbor),
    };

    out.or_panic()
}

pub type DownstreamPort = gasket::messaging::OutputPort<PullEvent>;

pub enum WorkUnit {
    Pull,
    Await,
}

#[derive(Stage)]
#[stage(name = "pull", unit = "WorkUnit", worker = "Worker")]
pub struct Stage {
    peer_session: Arc<Mutex<PeerClient>>,
    intersection: Vec<Point>,

    pub downstream: DownstreamPort,

    #[metric]
    chain_tip: gasket::metrics::Gauge,
}

impl Stage {
    pub fn new(peer_session: Arc<Mutex<PeerClient>>, intersection: Vec<Point>) -> Self {
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
}

pub struct Worker {}

#[async_trait::async_trait(?Send)]
impl gasket::framework::Worker<Stage> for Worker {
    async fn bootstrap(stage: &Stage) -> Result<Self, WorkerError> {
        let mut peer_session = stage.peer_session.lock().await;
        let client = (*peer_session).chainsync();

        let span_intersect =
            info_span!(target: EVENT_TARGET, "intersect", point = ?stage.intersection).entered();

        let (point, _) = client
            .find_intersect(stage.intersection.clone())
            .await
            .or_restart()?;

        span_intersect.exit();

        let _intersection = point.ok_or(miette!("couldn't find intersect")).or_panic()?;

        let worker = Self {};

        Ok(worker)
    }

    async fn schedule(&mut self, stage: &mut Stage) -> Result<WorkSchedule<WorkUnit>, WorkerError> {
        let mut peer_session = stage.peer_session.lock().await;
        let client = (*peer_session).chainsync();

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
            let mut peer_session = stage.peer_session.lock().await;
            let client = (*peer_session).chainsync();

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
            NextResponse::RollForward(header, tip) => {
                let header = to_traverse(&header).or_panic()?;
                let point = Point::Specific(header.slot(), header.hash().to_vec());

                let raw_header: RawHeader = header.cbor().to_vec();

                stage
                    .downstream
                    .send(PullEvent::RollForward(point, raw_header).into())
                    .await
                    .or_panic()?;

                stage.track_tip(&tip);
            }
            NextResponse::RollBackward(point, tip) => {
                stage
                    .downstream
                    .send(PullEvent::Rollback(point).into())
                    .await
                    .or_panic()?;

                stage.track_tip(&tip);
            }
            NextResponse::Await => {}
        };

        Ok(())
    }
}
