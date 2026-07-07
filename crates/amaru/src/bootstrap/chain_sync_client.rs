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

use amaru_kernel::{Peer, Point};
use amaru_network::point::{from_network_point, to_network_point};
use amaru_observability::debug_span;
use pallas_network::miniprotocols::chainsync::{Client, ClientError, HeaderContent, NextResponse};
use tracing::Instrument;

/// Handles chain synchronization network operations
pub struct ChainSyncClient {
    pub peer: Peer,
    chain_sync: Client<HeaderContent>,
    intersection: Vec<Point>,
}

impl ChainSyncClient {
    pub fn new(peer: Peer, chain_sync: Client<HeaderContent>, intersection: Vec<Point>) -> Self {
        Self { peer, chain_sync, intersection }
    }

    pub async fn find_intersection(&mut self) -> Result<Point, ChainSyncClientError> {
        async {
            let client = &mut self.chain_sync;
            let (point, _) = client
                .find_intersect(self.intersection.iter().cloned().map(to_network_point).collect())
                .await
                .map_err(ChainSyncClientError::NetworkError)?;

            let intersection =
                point.ok_or(ChainSyncClientError::NoIntersectionFound { points: self.intersection.clone() })?;
            Ok(from_network_point(&intersection))
        }
        .instrument(debug_span!(
            amaru::consensus::chain::FIND_INTERSECTION,
            peer = &self.peer.name,
            intersection_slot = u64::from(self.intersection.last().map(|p| p.slot_or_default()).unwrap_or_default())
        ))
        .await
    }

    pub async fn request_next(&mut self) -> Result<NextResponse<HeaderContent>, ChainSyncClientError> {
        let client = &mut self.chain_sync;

        client
            .request_next()
            .await
            .inspect_err(|err| tracing::error!(reason = %err, "request next failed"))
            .map_err(ChainSyncClientError::NetworkError)
    }

    pub async fn await_next(&mut self) -> Result<NextResponse<HeaderContent>, ChainSyncClientError> {
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

#[derive(Debug, thiserror::Error)]
pub enum ChainSyncClientError {
    #[error("Network error: {0}")]
    NetworkError(ClientError),
    #[error("No intersection found for points: {points:?}")]
    NoIntersectionFound { points: Vec<Point> },
}
