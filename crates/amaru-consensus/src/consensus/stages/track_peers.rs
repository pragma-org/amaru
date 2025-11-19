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
use crate::consensus::effects::BaseOps;
use crate::consensus::effects::ConsensusOps;
use crate::consensus::span::HasSpan;
use amaru_kernel::consensus_events::{DecodedChainSyncEvent, Tracked};
use amaru_kernel::peer::Peer;
use pure_stage::StageRef;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fmt::Debug};
use tracing::{Instrument, debug, trace};

type State = (SyncTracker, StageRef<DecodedChainSyncEvent>);

pub fn stage(
    state: State,
    msg: Tracked<DecodedChainSyncEvent>,
    eff: impl ConsensusOps,
) -> impl Future<Output = State> {
    let span = tracing::trace_span!(parent: msg.span(), "chain_sync.track_peers");
    async move {
        let (mut tracker, downstream) = state;
        match msg {
            Tracked::Wrapped(e @ DecodedChainSyncEvent::RollForward { .. }) => {
                if tracker.is_caught_up() {
                    debug!(target: EVENT_TARGET, peer = %e.peer(), point = %e.point(), "track_peers.caught_up.new_tip");
                } else {
                    trace!(target: EVENT_TARGET, peer= %e.peer(), point = %e.point(), "track_peers.syncing.new_tip");
                };
                eff.base().send(&downstream, e).await;
            }
            Tracked::Wrapped(e @ DecodedChainSyncEvent::Rollback { .. }) => {
                eff.base().send(&downstream, e).await;
            }
            Tracked::CaughtUp { peer, .. } => tracker.caught_up(&peer),
        }
        (tracker, downstream)
    }
    .instrument(span)
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
    use std::collections::BTreeMap;

    use crate::consensus::effects::mock_consensus_ops;

    use super::*;
    use amaru_kernel::{
        IsHeader,
        is_header::tests::{any_header, run},
        peer::Peer,
    };
    use tracing::Span;

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

    #[tokio::test]
    async fn roll_forward_is_sent_to_downstream() -> anyhow::Result<()> {
        let message = DecodedChainSyncEvent::RollForward {
            peer: Peer::new("alice"),
            header: run(any_header()),
            span: Span::current(),
        };
        let consensus_ops = mock_consensus_ops();

        stage(
            make_state(),
            Tracked::Wrapped(message.clone()),
            consensus_ops.clone(),
        )
        .await;
        assert_eq!(
            consensus_ops.mock_base.received(),
            BTreeMap::from_iter(vec![(
                "downstream".to_string(),
                vec![format!("{message:?}")]
            )])
        );
        Ok(())
    }

    #[tokio::test]
    async fn rollback_is_sent_to_downstream() -> anyhow::Result<()> {
        let message = DecodedChainSyncEvent::Rollback {
            peer: Peer::new("alice"),
            rollback_point: run(any_header()).point(),
            span: Span::current(),
        };
        let consensus_ops = mock_consensus_ops();

        stage(
            make_state(),
            Tracked::Wrapped(message.clone()),
            consensus_ops.clone(),
        )
        .await;
        assert_eq!(
            consensus_ops.mock_base.received(),
            BTreeMap::from_iter(vec![(
                "downstream".to_string(),
                vec![format!("{message:?}")]
            )])
        );
        Ok(())
    }

    fn make_state() -> State {
        let alice = Peer::new("alice");
        let bob = Peer::new("bob");
        let peers = vec![alice.clone(), bob.clone()];
        let tracker = SyncTracker::new(&peers);
        let downstream: StageRef<DecodedChainSyncEvent> = StageRef::named("downstream");
        (tracker, downstream)
    }
}
