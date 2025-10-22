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

use crate::consensus::EVENT_TARGET;
use crate::consensus::effects::ConsensusOps;
use crate::consensus::span::HasSpan;
use amaru_kernel::consensus_events::ChainSyncEvent;
use amaru_kernel::peer::Peer;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fmt::Debug};
use tracing::{Level, debug, span, trace};

type State = SyncTracker;

pub async fn stage(mut state: State, msg: ChainSyncEvent, _eff: impl ConsensusOps) -> State {
    let span = span!(parent: msg.span(), Level::TRACE, "stage.track_peers");
    let _entered = span.enter();

    match msg {
        ChainSyncEvent::RollForward { peer, point, .. } => {
            if state.is_caught_up() {
                debug!(target: EVENT_TARGET, %peer, point = %point, "new tip");
            } else {
                trace!(target: EVENT_TARGET, %peer, point = %point, "new tip");
            }
        }
        ChainSyncEvent::CaughtUp { peer, .. } => state.caught_up(&peer),
        ChainSyncEvent::Rollback { .. } => {}
    }
    state
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct SyncTracker {
    syncing_peers: BTreeSet<Peer>,
}

impl SyncTracker {
    pub fn new(peers: &[Peer]) -> Self {
        let syncing_peers = peers.iter().cloned().collect();
        Self { syncing_peers }
    }

    pub fn caught_up(&mut self, peer: &Peer) {
        self.syncing_peers.remove(peer);
    }

    pub fn is_caught_up(&self) -> bool {
        self.syncing_peers.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub enum SyncState {
    Syncing,
    CaughtUp,
}

#[cfg(test)]
mod tests {
    use super::*;
    use amaru_kernel::peer::Peer;

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
