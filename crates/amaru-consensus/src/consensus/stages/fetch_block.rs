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

use crate::consensus::effects::block_effects::FetchBlockEffect;
use crate::consensus::errors::{ConsensusError, ValidationFailed};
use crate::consensus::events::{ValidateBlockEvent, ValidateHeaderEvent};
use crate::consensus::span::adopt_current_span;
use amaru_kernel::{Point, RawBlock, peer::Peer};
use amaru_ouroboros_traits::{CanFetchBlock, IsHeader};
use async_trait::async_trait;
use pure_stage::{Effects, StageRef};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{Level, error, instrument};

type State = (StageRef<ValidateBlockEvent>, StageRef<ValidationFailed>);

/// This stages fetches the full block from a peer after its header has been validated.
/// It then sends the full block to the downstream stage for validation and storage.
#[instrument(
    level = Level::TRACE,
    skip_all,
    name = "stage.fetch_block",
)]
pub async fn stage(
    (downstream, errors): State,
    msg: ValidateHeaderEvent,
    eff: Effects<ValidateHeaderEvent>,
) -> State {
    adopt_current_span(&msg);
    match msg {
        ValidateHeaderEvent::Validated { peer, header, span } => {
            let point = header.point();
            match eff
                .external(FetchBlockEffect::new(peer.clone(), point.clone()))
                .await
            {
                Ok(block) => {
                    let block = RawBlock::from(&*block);
                    eff.send(
                        &downstream,
                        ValidateBlockEvent::Validated {
                            peer,
                            header,
                            block,
                            span,
                        },
                    )
                    .await
                }
                Err(e) => eff.send(&errors, ValidationFailed::new(&peer, e)).await,
            }
        }
        ValidateHeaderEvent::Rollback {
            peer,
            rollback_point,
            span,
            ..
        } => {
            eff.send(
                &downstream,
                ValidateBlockEvent::Rollback {
                    peer,
                    rollback_point,
                    span,
                },
            )
            .await
        }
    }
    (downstream, errors)
}

/// A trait for fetching blocks from peers.
#[async_trait]
pub trait BlockFetcher {
    async fn fetch_block(&self, peer: &Peer, point: &Point) -> Result<Vec<u8>, ConsensusError>;
}

/// This is a map of clients used to fetch blocks, with one client per peer.
pub struct ClientsBlockFetcher {
    clients: RwLock<BTreeMap<Peer, Arc<dyn CanFetchBlock>>>,
}

impl ClientsBlockFetcher {
    /// Retrieve a block from a peer at a given point.
    async fn fetch(&self, peer: &Peer, point: &Point) -> Result<Vec<u8>, ConsensusError> {
        // FIXME: should not fail if the peer is not found
        // the block should be fetched from any other valid peer
        // which is known to have it
        let client = {
            let clients = self.clients.read().await;
            clients
                .get(peer)
                .cloned()
                .ok_or_else(|| ConsensusError::UnknownPeer(peer.clone()))?
        };
        client
            .fetch_block(point)
            .await
            .map_err(|e| {
                error!(target: "amaru::consensus", "failed to fetch block from peer {}: {}", peer.name, e);
                ConsensusError::FetchBlockFailed(point.clone())
            })
    }
}

impl ClientsBlockFetcher {
    pub fn new(clients: Vec<(Peer, Arc<dyn CanFetchBlock>)>) -> Self {
        let mut cs = BTreeMap::new();
        for (peer, client) in clients {
            cs.insert(peer, client);
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
