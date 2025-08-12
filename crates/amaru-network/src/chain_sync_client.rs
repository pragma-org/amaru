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

use crate::{point::to_network_point, session::PeerSession};
use amaru_consensus::{RawHeader, consensus::ChainSyncEvent};
use amaru_kernel::Point;
use pallas_network::miniprotocols::chainsync::{ClientError, HeaderContent, NextResponse, Tip};
use pallas_traverse::MultiEraHeader;
use std::sync::{Arc, RwLock};
use tracing::{instrument, Level, Span};
use pallas_network::miniprotocols::Point as NetworkPoint;

const MAX_BATCH_SIZE : usize = 10;

#[derive(Debug, thiserror::Error)]
pub enum ChainSyncClientError {
    #[error("Failed to decode header: {0}")]
    HeaderDecodeError(String),
    #[error("Network error: {0}")]
    NetworkError(ClientError),
    #[error("No intersection found for points: {points:?}")]
    NoIntersectionFound { points: Vec<Point> },
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

struct PullBuffer {
    buffer: Vec<HeaderContent>
}

impl PullBuffer {
    fn new() -> Self {
        PullBuffer { buffer: vec![] }
    }

    fn len(&self) -> usize {
        self.buffer.len()
    }

    fn forward(&mut self, header: HeaderContent) {
        self.buffer.push(header)
    }

    fn rollback(&mut self, _point: &NetworkPoint) -> RollbackHandling {
        RollbackHandling::Handled
    }
}

enum RollbackHandling {
    Handled,
    BeforeBatch
}

pub enum PullResult {
    ForwardBatch(Vec<HeaderContent>),
    RollBack(Point),
    Nothing
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
    fn no_longer_catching_up(&self) {
        // Do not acquire the lock unless necessary.
        if self.is_catching_up.read().map(|lock| *lock).unwrap_or(true) {
            tracing::info!("chain tip reached; awaiting next block");
            *self.is_catching_up.write().unwrap() = false;
        }
    }

    #[allow(clippy::unwrap_used)]
    fn catching_up(&self) {
        // Do not acquire the lock unless necessary.
        if !self
            .is_catching_up
            .read()
            .map(|lock| *lock)
            .unwrap_or(false)
        {
            *self.is_catching_up.write().unwrap() = true;
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
            .map_err(ChainSyncClientError::NetworkError)?;

        point.ok_or(ChainSyncClientError::NoIntersectionFound {
            points: self.intersection.clone(),
        })?;
        Ok(())
    }

    pub fn roll_forward(
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

    pub fn roll_back(
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

    pub async fn pull_batch(
        &mut self,
    ) -> Result<PullResult, ChainSyncClientError> {
        self.catching_up();
        let mut peer_client = self.peer_session.lock().await;
        let client = (*peer_client).chainsync();
        let mut batch = PullBuffer::new();

        while batch.len() < MAX_BATCH_SIZE {
            let response = client.recv_while_can_await().await.map_err(|_| unimplemented!())?;

            match response {
                NextResponse::RollForward(content, _tip) => batch.forward(content),
                NextResponse::RollBackward(point, _tip) => match batch.rollback(&point) {
                    RollbackHandling::Handled => (),
                    RollbackHandling::BeforeBatch => return Ok(PullResult::RollBack(crate::point::from_network_point(&point)))
                }
                NextResponse::Await => break,
            }
        };

        Ok(PullResult::Nothing)
    }

    pub async fn await_next(
        &mut self,
    ) -> Result<NextResponse<HeaderContent>, ChainSyncClientError> {
        self.no_longer_catching_up();
        let mut peer_client = self.peer_session.lock().await;
        let client = (*peer_client).chainsync();

        match client.recv_while_must_reply().await {
            Ok(result) => Ok(result),
            Err(e) => {
                tracing::error!(reason = %e, "failed while awaiting for next block");
                Err(ChainSyncClientError::NetworkError(e))
            }
        }
    }

    pub async fn has_agency(&self) -> bool {
        let mut peer_client = self.peer_session.lock().await;
        let client = (*peer_client).chainsync();
        client.has_agency()
    }
}
