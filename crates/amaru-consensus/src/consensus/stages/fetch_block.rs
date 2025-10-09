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

use crate::consensus::effects::{BaseOps, ConsensusOps, NetworkOps};
use crate::consensus::errors::{ConsensusError, ValidationFailed};
use crate::consensus::events::{ValidateBlockEvent, ValidateHeaderEvent};
use crate::consensus::span::HasSpan;
use amaru_kernel::{Point, RawBlock, peer::Peer};
use amaru_ouroboros_traits::{CanFetchBlock, IsHeader};
use async_trait::async_trait;
use pure_stage::StageRef;
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{Level, error, span};

type State = (StageRef<ValidateBlockEvent>, StageRef<ValidationFailed>);

/// This stages fetches the full block from a peer after its header has been validated.
/// It then sends the full block to the downstream stage for validation and storage.
pub async fn stage(
    (downstream, errors): State,
    msg: ValidateHeaderEvent,
    eff: impl ConsensusOps,
) -> State {
    let span = span!(parent: msg.span(), Level::TRACE, "stage.fetch_block");
    let _entered = span.enter();

    match msg {
        ValidateHeaderEvent::Validated { peer, header, span } => {
            let point = header.point();
            match eff.network().fetch_block(&peer, &point).await {
                Ok(block) => {
                    let block = RawBlock::from(&*block);
                    eff.base()
                        .send(
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
                Err(e) => {
                    eff.base()
                        .send(&errors, ValidationFailed::new(&peer, e))
                        .await
                }
            }
        }
        ValidateHeaderEvent::Rollback {
            peer,
            rollback_header: rollback_point,
            span,
            ..
        } => {
            eff.base()
                .send(
                    &downstream,
                    ValidateBlockEvent::Rollback {
                        peer,
                        rollback_header: rollback_point,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::effects::mock_consensus_ops;
    use crate::consensus::errors::ValidationFailed;
    use crate::consensus::tests::any_header;
    use amaru_kernel::peer::Peer;
    use amaru_ouroboros_traits::fake::tests::run;
    use pure_stage::StageRef;
    use std::collections::BTreeMap;
    use tracing::Span;

    #[tokio::test]
    async fn a_block_that_can_be_fetched_is_sent_downstream() -> anyhow::Result<()> {
        let peer = Peer::new("name");
        let header = run(any_header());
        let message = ValidateHeaderEvent::Validated {
            peer: peer.clone(),
            header: header.clone(),
            span: Span::current(),
        };
        let block = vec![1u8; 128];
        let consensus_ops = mock_consensus_ops();
        consensus_ops.mock_network.return_block(Ok(block.clone()));

        stage(make_state(), message, consensus_ops.clone()).await;

        let forwarded = ValidateBlockEvent::Validated {
            peer: peer.clone(),
            header,
            block: RawBlock::from(block.as_slice()),
            span: Span::current(),
        };
        assert_eq!(
            consensus_ops.mock_base.received(),
            BTreeMap::from_iter(vec![(
                "downstream".to_string(),
                vec![format!("{forwarded:?}")]
            )])
        );
        Ok(())
    }

    // HELPERS

    fn make_state() -> State {
        let downstream: StageRef<ValidateBlockEvent> = StageRef::named("downstream");
        let errors: StageRef<ValidationFailed> = StageRef::named("errors");
        (downstream, errors)
    }
}
