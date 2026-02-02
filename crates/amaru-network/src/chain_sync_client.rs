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

use crate::point::{from_network_point, to_network_point};
use amaru_kernel::{Peer, Point};
use amaru_observability::trace;
use pallas_network::miniprotocols::chainsync::{Client, ClientError, HeaderContent, NextResponse};
use pallas_traverse::MultiEraHeader;

pub type RawHeader = Vec<u8>;

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
    pub peer: Peer,
    chain_sync: Client<HeaderContent>,
    intersection: Vec<Point>,
}

impl ChainSyncClient {
    pub fn new(peer: Peer, chain_sync: Client<HeaderContent>, intersection: Vec<Point>) -> Self {
        Self {
            peer,
            chain_sync,
            intersection,
        }
    }

    #[trace(amaru::network::chainsync_client::FIND_INTERSECTION,
        peer = self.peer.name.clone(),
        intersection_slot = u64::from(self.intersection.last().map(|p| p.slot_or_default()).unwrap_or_default())
    )]
    pub async fn find_intersection(&mut self) -> Result<Point, ChainSyncClientError> {
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

        let intersection = point.ok_or(ChainSyncClientError::NoIntersectionFound {
            points: self.intersection.clone(),
        })?;
        Ok(from_network_point(&intersection))
    }

    pub fn intersection(&self) -> &[Point] {
        &self.intersection
    }

    pub async fn request_next(
        &mut self,
    ) -> Result<NextResponse<HeaderContent>, ChainSyncClientError> {
        let client = &mut self.chain_sync;

        client
            .request_next()
            .await
            .inspect_err(|err| tracing::error!(reason = %err, "request next failed"))
            .map_err(ChainSyncClientError::NetworkError)
    }

    pub async fn await_next(
        &mut self,
    ) -> Result<NextResponse<HeaderContent>, ChainSyncClientError> {
        let client = &mut self.chain_sync;

        match client.recv_while_must_reply().await {
            Ok(result) => Ok(result),
            Err(err) => {
                tracing::error!(reason = %err, "failed while awaiting for next block");
                Err(ChainSyncClientError::NetworkError(err))
            }
        }
    }

    pub fn has_agency(&self) -> bool {
        self.chain_sync.has_agency()
    }
}
