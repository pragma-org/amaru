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

use amaru_kernel::{Point, peer::Peer};
use amaru_ouroboros_traits::{BlockFetchClientError, CanFetchBlock};
use anyhow::anyhow;
use async_trait::async_trait;
use pallas_network::miniprotocols;
use pallas_network::miniprotocols::blockfetch::Client;
use tokio::sync::Mutex;
use tracing::{Level, instrument};

pub type RawHeader = Vec<u8>;

/// A BlockFetchClient encapsulates a connection to a peer for fetching blocks.
/// The BlockFetchClient uses the amaru Point type to specify which block to fetch in its API,
/// instead of the underlying miniprotocols Point type in order to decouple the rest of the codebase
/// from the pallas_network crate.
pub struct PallasBlockFetchClient {
    pub peer: Peer,
    client: Mutex<Client>,
}

impl PallasBlockFetchClient {
    pub fn new(peer: &Peer, client: Client) -> Self {
        Self {
            peer: peer.clone(),
            client: Mutex::new(client),
        }
    }

    #[instrument(
        level = Level::TRACE,
        skip_all,
        name = "blockfetch_client.fetch_block",
        fields(
            peer = self.peer.name,
            point = %point,
        ),
    )]
    pub async fn fetch_block(&self, point: &Point) -> Result<Vec<u8>, BlockFetchClientError> {
        let point: miniprotocols::Point = match point.clone() {
            Point::Origin => miniprotocols::Point::Origin,
            Point::Specific(slot, hash) => miniprotocols::Point::Specific(slot, hash),
        };
        let mut client = self.client.lock().await;
        client
            .fetch_single(point)
            .await
            .map_err(|e| BlockFetchClientError::new(anyhow!(e)))
    }
}

#[async_trait]
impl CanFetchBlock for PallasBlockFetchClient {
    async fn fetch_block(
        &self,
        _peer: &Peer,
        point: &Point,
    ) -> Result<Vec<u8>, BlockFetchClientError> {
        self.fetch_block(point).await
    }
}
