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

use crate::point::to_network_point;
use amaru_consensus::{RawHeader, consensus::ChainSyncEvent};
use amaru_kernel::{Point, peer::Peer};
use pallas_network::miniprotocols::chainsync::{
    Client, ClientError, HeaderContent, NextResponse, Tip,
};
use pallas_traverse::MultiEraHeader;
use std::sync::{Arc, RwLock};
use tracing::{Level, Span, instrument};

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
    peer: Peer,
    chain_sync: Client<HeaderContent>,
    intersection: Vec<Point>,
    is_catching_up: Arc<RwLock<bool>>,
}

impl ChainSyncClient {
    pub fn new(
        peer: Peer,
        chain_sync: Client<HeaderContent>,
        intersection: Vec<Point>,
        is_catching_up: Arc<RwLock<bool>>,
    ) -> Self {
        Self {
            peer,
            chain_sync,
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
            peer = self.peer.name,
            intersection.slot = %self.intersection.last().map(|p| p.slot_or_default()).unwrap_or_default(),
        ),
    )]
    pub async fn find_intersection(&mut self) -> Result<(), ChainSyncClientError> {
        let client = &mut self.chain_sync;
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

    pub async fn roll_forward(
        &mut self,
        header: &HeaderContent,
    ) -> Result<ChainSyncEvent, ChainSyncClientError> {
        let peer = &self.peer;
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
        let peer = &self.peer;
        Ok(ChainSyncEvent::Rollback {
            peer: peer.clone(),
            rollback_point,
            span: Span::current(),
        })
    }

    pub async fn request_next(
        &mut self,
    ) -> Result<NextResponse<HeaderContent>, ChainSyncClientError> {
        self.catching_up();
        let client = &mut self.chain_sync;

        client
            .request_next()
            .await
            .inspect_err(|err| tracing::error!(reason = %err, "request next failed; retrying"))
            .map_err(ChainSyncClientError::NetworkError)
    }

    pub async fn await_next(
        &mut self,
    ) -> Result<NextResponse<HeaderContent>, ChainSyncClientError> {
        self.no_longer_catching_up();
        let client = &mut self.chain_sync;

        match client.recv_while_must_reply().await {
            Ok(result) => Ok(result),
            Err(e) => {
                tracing::error!(reason = %e, "failed while awaiting for next block");
                Err(ChainSyncClientError::NetworkError(e))
            }
        }
    }

    pub async fn has_agency(&self) -> bool {
        let client = &self.chain_sync;
        client.has_agency()
    }
}
