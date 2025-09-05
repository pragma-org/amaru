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

use amaru_consensus::ConsensusError;
use amaru_consensus::consensus::fetch_block::BlockFetcher;
use amaru_kernel::Point;
use amaru_kernel::peer::Peer;
use async_trait::async_trait;
use pallas_network::miniprotocols::blockfetch::Client;
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

pub struct ClientsBlockFetcher {
    clients: RwLock<BTreeMap<Peer, Arc<Mutex<Client>>>>,
}

impl ClientsBlockFetcher {
    async fn fetch(&self, peer: &Peer, point: &Point) -> Result<Vec<u8>, ConsensusError> {
        // FIXME: should not crash if the peer is not found
        // the block should be fetched from any other valid peer
        // which is known to have it
        let client = {
            let clients = self.clients.read().await;
            clients
                .get(peer)
                .cloned()
                .ok_or_else(|| ConsensusError::UnknownPeer(peer.clone()))?
        };
        let mut client = client.lock().await;
        let new_point: pallas_network::miniprotocols::Point = match point.clone() {
            Point::Origin => pallas_network::miniprotocols::Point::Origin,
            Point::Specific(slot, hash) => {
                pallas_network::miniprotocols::Point::Specific(slot, hash)
            }
        };
        client
            .fetch_single(new_point)
            .await
            .map_err(|_| ConsensusError::FetchBlockFailed(point.clone()))
    }
}

impl ClientsBlockFetcher {
    pub fn new(clients: Vec<(Peer, Client)>) -> Self {
        let mut cs = BTreeMap::new();
        for (peer, client) in clients {
            cs.insert(peer, Arc::new(Mutex::new(client)));
        }
        Self {
            clients: RwLock::new(cs),
        }
    }
}

#[async_trait]
impl BlockFetcher for ClientsBlockFetcher {
    async fn fetch_block(&self, peer: &Peer, point: &Point) -> Result<Vec<u8>, ConsensusError> {
        self.fetch(peer, point).await
    }
}
