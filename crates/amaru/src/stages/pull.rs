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
    point::{from_network_point, to_network_point},
    send,
    stages::PeerSession,
};
use amaru_consensus::{consensus::ChainSyncEvent, RawHeader};
use amaru_kernel::Point;
use gasket::framework::*;
use pallas_network::miniprotocols::chainsync::{HeaderContent, NextResponse, Tip};
use pallas_traverse::MultiEraHeader;
use std::sync::{Arc, RwLock};
use tracing::{instrument, Level, Span};

#[derive(Debug, thiserror::Error)]
pub enum ChainSyncClientError {
    #[error("Failed to decode header: {0}")]
    HeaderDecodeError(String),
    #[error("Network error: {0}")]
    NetworkError(String),
    #[error("No intersection found for points: {points:?}")]
    NoIntersectionFound { points: Vec<Point> },
    #[error("Peer client lock error")]
    PeerClientLockError,
}

pub fn to_traverse(header: &HeaderContent) -> Result<MultiEraHeader<'_>, ChainSyncClientError> {
    let out = match header.byron_prefix {
        Some((subtag, _)) => MultiEraHeader::decode(header.variant, Some(subtag), &header.cbor),
        None => MultiEraHeader::decode(header.variant, None, &header.cbor),
    };

    out.map_err(|e| ChainSyncClientError::HeaderDecodeError(e.to_string()))
}

/// Handles chain synchronization network operations
pub struct ChainSyncClient {
    peer_session: PeerSession,
    intersection: Vec<Point>,
    is_catching_up: Arc<RwLock<bool>>,
}

impl ChainSyncClient {
    pub fn new(
        peer_session: PeerSession,
        intersection: Vec<Point>,
        is_catching_up: Arc<RwLock<bool>>,
    ) -> Self {
        Self {
            peer_session,
            intersection,
            is_catching_up,
        }
    }

    #[allow(clippy::unwrap_used)]
    fn no_longer_catching_up(is_catching_up: &RwLock<bool>) {
        // Do not acquire the lock unless necessary.
        if is_catching_up.read().map(|lock| *lock).unwrap_or(true) {
            tracing::info!("chain tip reached; awaiting next block");
            *is_catching_up.write().unwrap() = false;
        }
    }

    #[allow(clippy::unwrap_used)]
    fn catching_up(is_catching_up: &RwLock<bool>) {
        // Do not acquire the lock unless necessary.
        if !is_catching_up.read().map(|lock| *lock).unwrap_or(false) {
            *is_catching_up.write().unwrap() = true;
        }
    }

    #[instrument(
        level = Level::TRACE,
        skip_all,
        name = "chainsync_client.find_intersection",
        fields(
            peer = self.peer_session.peer.name,
            intersection.slot = %self.intersection.last().map(|p| p.slot_or_default()).unwrap_or_default(),
        ),
    )]
    pub async fn find_intersection(&self) -> Result<(), ChainSyncClientError> {
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
            .map_err(|e| ChainSyncClientError::NetworkError(e.to_string()))?;

        let _intersection = point.ok_or(ChainSyncClientError::NoIntersectionFound {
            points: self.intersection.clone(),
        })?;
        Ok(())
    }

    pub async fn roll_forward(
        &mut self,
        header: &HeaderContent,
    ) -> Result<ChainSyncEvent, ChainSyncClientError> {
        let peer = &self.peer_session.peer;
        let header = to_traverse(header)?;
        let point = Point::Specific(header.slot(), header.hash().to_vec());

        let raw_header: RawHeader = header.cbor().to_vec();

        let event = ChainSyncEvent::RollForward {
            peer: peer.clone(),
            point,
            raw_header,
            span: Span::current(),
        };

        Ok(event)
    }

    pub async fn roll_back(
        &self,
        rollback_point: Point,
        _tip: Tip,
    ) -> Result<ChainSyncEvent, ChainSyncClientError> {
        let peer = &self.peer_session.peer;
        Ok(ChainSyncEvent::Rollback {
            peer: peer.clone(),
            rollback_point,
            span: Span::current(),
        })
    }

    pub async fn request_next(
        &mut self,
    ) -> Result<NextResponse<HeaderContent>, ChainSyncClientError> {
        Self::catching_up(&self.is_catching_up);
        let mut peer_client = self.peer_session.lock().await;
        let client = (*peer_client).chainsync();

        client
            .request_next()
            .await
            .inspect_err(|err| tracing::error!(reason = %err, "request next failed; retrying"))
            .map_err(|e| ChainSyncClientError::NetworkError(e.to_string()))
    }

    pub async fn await_next(
        &mut self,
    ) -> Result<NextResponse<HeaderContent>, ChainSyncClientError> {
        Self::no_longer_catching_up(&self.is_catching_up);
        let mut peer_client = self.peer_session.lock().await;
        let client = (*peer_client).chainsync();

        match client.recv_while_must_reply().await {
            Ok(result) => Ok(result),
            Err(err) => {
                tracing::error!(reason = %err, "failed while awaiting for next block");
                Err(ChainSyncClientError::NetworkError(err.to_string()))
            }
        }
    }

    pub async fn has_agency(&self) -> bool {
        let mut peer_client = self.peer_session.peer_client.lock().await;
        let client = (*peer_client).chainsync();
        client.has_agency()
    }
}

pub type DownstreamPort = gasket::messaging::OutputPort<ChainSyncEvent>;

pub enum WorkUnit {
    Pull,
    Await,
}

#[derive(Stage)]
#[stage(name = "stage.chain_sync_client", unit = "WorkUnit", worker = "Worker")]
pub struct Stage {
    pub client: ChainSyncClient,
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

    pub async fn find_intersection(&self) -> Result<(), WorkerError> {
        self.client
            .find_intersection()
            .await
            .or_panic()
    }

    pub async fn roll_forward(&mut self, header: &HeaderContent) -> Result<(), WorkerError> {
        let event = self
            .client
            .roll_forward(header)
            .await
            .or_panic();
        send!(&mut self.downstream, event)
    }

    pub async fn roll_back(&mut self, rollback_point: Point, tip: Tip) -> Result<(), WorkerError> {
        let event = self
            .client
            .roll_back(rollback_point, tip)
            .await
            .or_panic();
        self.downstream.send(event.into()).await.or_panic()
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
        if stage.client.has_agency().await {
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
        let next = match unit {
            WorkUnit::Pull => stage
                .client
                .request_next()
                .await
                .or_panic()?,
            WorkUnit::Await => stage
                .client
                .await_next()
                .await
                .or_panic()?,
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
