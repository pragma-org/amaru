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

use crate::consensus::events::ChainSyncEvent;
use amaru_kernel::Peer;
use amaru_protocols::chainsync::{self, ChainSyncInitiatorMsg};
use pure_stage::{Effects, StageRef};
use std::collections::BTreeSet;
use tracing::{Span, instrument};

#[instrument(level = "debug", name = "diffusion.chain_sync", skip_all)]
pub async fn stage(
    (mut tracker, downstream): (SyncTracker, StageRef<ChainSyncEvent>),
    msg: ChainSyncInitiatorMsg,
    eff: Effects<ChainSyncInitiatorMsg>,
) -> (SyncTracker, StageRef<ChainSyncEvent>) {
    use chainsync::InitiatorResult::*;
    match msg.msg {
        Initialize => {
            tracing::info!(peer = %msg.peer,"initializing chainsync");
            tracker.add_peer(msg.peer);
        }
        IntersectFound(point, tip) => {
            tracing::info!(peer = %msg.peer, %point, tip_point = %tip.point(), "intersect found");
        }
        IntersectNotFound(tip) => {
            tracing::info!(peer = %msg.peer, tip_point = %tip.point(), "intersect not found");
            eff.send(&msg.handler, chainsync::InitiatorMessage::Done)
                .await;
            tracker.caught_up(&msg.peer);
        }
        RollForward(header_content, tip) => {
            tracing::debug!(peer = %msg.peer, variant = header_content.variant,
                byron_prefix = ?header_content.byron_prefix, tip_point = %tip.point(), "roll forward");
            eff.send(&msg.handler, chainsync::InitiatorMessage::RequestNext)
                .await;
            eff.send(
                &downstream,
                ChainSyncEvent::RollForward {
                    peer: msg.peer,
                    tip,
                    raw_header: header_content.cbor,
                    span: Span::current(),
                },
            )
            .await;
        }
        RollBackward(point, tip) => {
            tracing::debug!(peer = %msg.peer, %point, tip_point = %tip.point(), "roll backward");
            eff.send(&msg.handler, chainsync::InitiatorMessage::RequestNext)
                .await;
            eff.send(
                &downstream,
                ChainSyncEvent::Rollback {
                    peer: msg.peer,
                    rollback_point: point,
                    tip,
                    span: Span::current(),
                },
            )
            .await;
        }
    }

    (tracker, downstream)
}

#[derive(Debug, Default, Clone, serde::Deserialize, serde::Serialize, PartialEq)]
pub struct SyncTracker {
    syncing_peers: BTreeSet<Peer>,
}

impl SyncTracker {
    pub fn new(peers: &[Peer]) -> Self {
        let syncing_peers = peers.iter().cloned().collect();
        Self { syncing_peers }
    }

    pub fn add_peer(&mut self, peer: Peer) {
        self.syncing_peers.insert(peer.clone());
    }

    pub fn caught_up(&mut self, peer: &Peer) {
        self.syncing_peers.remove(peer);
    }

    pub fn is_caught_up(&self) -> bool {
        self.syncing_peers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn is_caught_up_when_all_peers_are_caught_up() {
        let alice = Peer::new("alice");
        let bob = Peer::new("bob");
        let peers = vec![alice.clone(), bob.clone()];
        let mut tracker = SyncTracker::new(&peers);

        tracker.caught_up(&alice);
        tracker.caught_up(&bob);

        assert!(tracker.is_caught_up());
    }

    #[tokio::test]
    async fn is_not_caught_up_given_some_peer_is_not() {
        let alice = Peer::new("alice");
        let bob = Peer::new("bob");
        let peers = vec![alice.clone(), bob.clone()];
        let mut tracker = SyncTracker::new(&peers);

        tracker.caught_up(&alice);

        assert!(!tracker.is_caught_up());
    }
}
