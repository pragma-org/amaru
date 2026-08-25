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

use amaru_kernel::{BlockHeight, NetworkPoint, Peer, Point, Slot};
use amaru_observability::{Instrument, debug_span, error};
use pallas_network::miniprotocols::{
    Point as PallasPoint,
    chainsync::{Client, ClientError, HeaderContent, NextResponse, Tip as PallasTip},
};

// TODO: Avoid Pallas points here and use our own chain sync client.
fn to_pallas_point(point: NetworkPoint) -> PallasPoint {
    match point {
        NetworkPoint::Origin => PallasPoint::Origin,
        NetworkPoint::Specific(slot, hash) => PallasPoint::Specific(slot.as_u64(), hash.to_vec()),
    }
}

pub(crate) fn from_pallas_point(point: &PallasPoint) -> NetworkPoint {
    match point {
        PallasPoint::Origin => NetworkPoint::Origin,
        PallasPoint::Specific(slot, hash) => NetworkPoint::Specific(Slot::from(*slot), From::from(hash.as_slice())),
    }
}

pub(crate) fn from_pallas_tip(tip: &PallasTip) -> Point {
    from_pallas_point(&tip.0).with_height(BlockHeight::from(tip.1))
}

/// Handles chain synchronization network operations
pub struct ChainSyncClient {
    pub peer: Peer,
    chain_sync: Client<HeaderContent>,
    intersection: Vec<NetworkPoint>,
}

impl ChainSyncClient {
    pub fn new(peer: Peer, chain_sync: Client<HeaderContent>, intersection: Vec<NetworkPoint>) -> Self {
        Self { peer, chain_sync, intersection }
    }

    pub async fn find_intersection(&mut self) -> Result<NetworkPoint, ChainSyncClientError> {
        async {
            let client = &mut self.chain_sync;
            let (point, _) = client
                .find_intersect(self.intersection.iter().cloned().map(to_pallas_point).collect())
                .await
                .map_err(ChainSyncClientError::NetworkError)?;

            let intersection =
                point.ok_or(ChainSyncClientError::NoIntersectionFound { points: self.intersection.clone() })?;
            Ok(from_pallas_point(&intersection))
        }
        .instrument(debug_span!(
            amaru::consensus::chain::FIND_INTERSECTION,
            peer = &self.peer,
            intersection_slot = self.intersection.last().map(|p| p.slot_or_default()).unwrap_or_default()
        ))
        .await
    }

    pub async fn request_next(&mut self) -> Result<NextResponse<HeaderContent>, ChainSyncClientError> {
        let client = &mut self.chain_sync;

        client
            .request_next()
            .await
            .inspect_err(|err| {
                error!(bootstrap::headers::NEXT_FAILED, operation = "request_next", error = err.to_string());
            })
            .map_err(ChainSyncClientError::NetworkError)
    }

    pub async fn await_next(&mut self) -> Result<NextResponse<HeaderContent>, ChainSyncClientError> {
        let client = &mut self.chain_sync;

        match client.recv_while_must_reply().await {
            Ok(result) => Ok(result),
            Err(err) => {
                error!(bootstrap::headers::NEXT_FAILED, operation = "await_next", error = err.to_string());
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
    NoIntersectionFound { points: Vec<NetworkPoint> },
}
