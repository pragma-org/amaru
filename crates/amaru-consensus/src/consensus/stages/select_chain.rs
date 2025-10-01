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
use crate::consensus::effects::{BaseOps, ConsensusOps};
use crate::consensus::errors::{ConsensusError, ValidationFailed};
use crate::consensus::events::{DecodedChainSyncEvent, ValidateHeaderEvent};
use crate::consensus::headers_tree::{HeadersTree, HeadersTreeState};
use crate::consensus::span::adopt_current_span;
use amaru_kernel::{HEADER_HASH_SIZE, Header, Point, peer::Peer, string_utils::ListToString};
use amaru_ouroboros::IsHeader;
use amaru_ouroboros_traits::ChainStore;
use pallas_crypto::hash::Hash;
use pure_stage::{BoxFuture, StageRef};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::{
    collections::BTreeSet,
    fmt::{Debug, Display, Formatter},
    mem,
};
use tracing::{Level, Span, debug, info, instrument, trace};

pub const DEFAULT_MAXIMUM_FRAGMENT_LENGTH: usize = 2160;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct SelectChain {
    tree_state: HeadersTreeState,
    sync_tracker: SyncTracker,
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

impl SelectChain {
    pub fn new(tree_state: HeadersTreeState, peers: &[Peer]) -> Self {
        let sync_tracker = SyncTracker::new(peers);
        SelectChain {
            tree_state,
            sync_tracker,
        }
    }

    fn forward_block(peer: Peer, header: Header, span: Span) -> ValidateHeaderEvent {
        ValidateHeaderEvent::Validated { peer, header, span }
    }

    fn switch_to_fork(
        peer: Peer,
        rollback_header: Header,
        fork: Vec<Header>,
        span: Span,
    ) -> Vec<ValidateHeaderEvent> {
        let mut result = vec![ValidateHeaderEvent::Rollback {
            rollback_header,
            peer: peer.clone(),
            span: span.clone(),
        }];

        for header in fork {
            result.push(SelectChain::forward_block(
                peer.clone(),
                header,
                span.clone(),
            ));
        }

        result
    }

    pub async fn select_chain(
        &mut self,
        store: Arc<dyn ChainStore<Header>>,
        peer: Peer,
        header: Header,
        span: Span,
    ) -> Result<Vec<ValidateHeaderEvent>, ConsensusError> {
        // Temporarily take the tree state out of self, to avoid borrowing self
        let tree_state = mem::take(&mut self.tree_state);
        let mut headers_tree = HeadersTree::create(store, tree_state);
        let result = headers_tree.select_roll_forward(&peer, header)?;
        self.tree_state = headers_tree.into_tree_state();

        let events = match result {
            ForwardChainSelection::NewTip { peer, tip } => {
                if self.sync_tracker.is_caught_up() {
                    debug!(target: EVENT_TARGET, %peer, hash = %tip.hash(), slot = %tip.slot(), "new tip");
                } else {
                    trace!(target: EVENT_TARGET, %peer, hash = %tip.hash(), slot = %tip.slot(), "new tip");
                }
                vec![SelectChain::forward_block(peer, tip, span)]
            }
            ForwardChainSelection::SwitchToFork(Fork {
                peer,
                rollback_header,
                fork,
            }) => {
                info!(target: EVENT_TARGET, rollback = %rollback_header.point(), length = fork.len(), "switching to fork");
                SelectChain::switch_to_fork(peer, rollback_header, fork, span)
            }
            ForwardChainSelection::NoChange => {
                trace!(target: EVENT_TARGET, "no change");
                vec![]
            }
        };

        Ok(events)
    }

    pub async fn select_rollback(
        &mut self,
        store: Arc<dyn ChainStore<Header>>,
        peer: Peer,
        rollback_point: Point,
        span: Span,
    ) -> Result<Vec<ValidateHeaderEvent>, ConsensusError> {
        // Temporarily take the tree state out of self, to avoid borrowing self
        let tree_state = mem::take(&mut self.tree_state);
        let mut headers_tree = HeadersTree::create(store, tree_state);
        let result = headers_tree.select_rollback(&peer, &rollback_point.hash())?;
        self.tree_state = headers_tree.into_tree_state();

        match result {
            RollbackChainSelection::SwitchToFork(Fork {
                peer,
                rollback_header,
                fork,
            }) => {
                info!(target: EVENT_TARGET, rollback = %rollback_header.point(), length = fork.len(), "switching to fork");
                Ok(SelectChain::switch_to_fork(
                    peer,
                    rollback_header,
                    fork,
                    span,
                ))
            }
            RollbackChainSelection::NoChange => Ok(vec![]),
            RollbackChainSelection::RollbackBeyondLimit {
                peer,
                rollback_point,
                max_point,
            } => Err(ConsensusError::InvalidRollback {
                peer,
                rollback_point,
                max_point,
            }),
        }
    }

    pub fn handle_chain_sync(
        &mut self,
        store: Arc<dyn ChainStore<Header>>,
        chain_sync: DecodedChainSyncEvent,
    ) -> BoxFuture<'_, Result<Vec<ValidateHeaderEvent>, ConsensusError>> {
        Box::pin(async move {
            match chain_sync {
                DecodedChainSyncEvent::RollForward {
                    peer, header, span, ..
                } => self.select_chain(store, peer, header, span).await,
                DecodedChainSyncEvent::Rollback {
                    peer,
                    rollback_point,
                    span,
                } => {
                    self.select_rollback(store, peer, rollback_point, span)
                        .await
                }
                DecodedChainSyncEvent::CaughtUp { peer, .. } => self.caught_up(&peer),
            }
        })
    }

    fn caught_up(&mut self, peer: &Peer) -> Result<Vec<ValidateHeaderEvent>, ConsensusError> {
        self.sync_tracker.caught_up(peer);
        Ok(vec![])
    }
}

/// Definition of a fork.
///
/// FIXME: The peer should not be needed here, as the fork should be
/// comprised of known blocks. It is only needed to download the blocks
/// we don't currently store.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Fork<H: IsHeader> {
    pub peer: Peer,
    pub rollback_header: H,
    pub fork: Vec<H>,
}

/// The outcome of the chain selection process in  case of
/// roll forward.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ForwardChainSelection<H: IsHeader> {
    /// The current best chain has been extended with a (single) new header.
    NewTip { peer: Peer, tip: H },

    /// The current best chain is unchanged.
    NoChange,

    /// The current best chain has switched to given fork.
    SwitchToFork(Fork<H>),
}

impl<H: IsHeader + Display> Display for ForwardChainSelection<H> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ForwardChainSelection::NewTip { peer, tip } => {
                write!(f, "NewTip[{}, {}]", peer, tip)
            }
            ForwardChainSelection::NoChange => f.write_str("NoChange"),
            ForwardChainSelection::SwitchToFork(Fork {
                peer,
                rollback_header: rollback_point,
                fork,
            }) => write!(
                f,
                "SwitchToFork[\n    peer: {},\n    rollback_point: {},\n    fork:\n        {}]",
                peer,
                rollback_point,
                fork.list_to_string(",\n        ")
            ),
        }
    }
}

/// The outcome of the chain selection process in case of rollback
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RollbackChainSelection<H: IsHeader> {
    /// The current best chain has switched to given fork.
    SwitchToFork(Fork<H>),

    /// The peer tried to rollback beyond the limit
    RollbackBeyondLimit {
        peer: Peer,
        rollback_point: Hash<HEADER_HASH_SIZE>,
        max_point: Hash<HEADER_HASH_SIZE>,
    },

    /// The current best chain has not changed
    NoChange,
}

impl<H: IsHeader + Display> Display for RollbackChainSelection<H> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            RollbackChainSelection::NoChange => f.write_str("NoChange"),
            RollbackChainSelection::SwitchToFork(Fork {
                peer,
                rollback_header: rollback_point,
                fork,
            }) => write!(
                f,
                "SwitchToFork[\n    peer: {},\n    rollback_point: {},\n    fork:\n        {}]",
                peer,
                rollback_point,
                fork.list_to_string(",\n        ")
            ),
            RollbackChainSelection::RollbackBeyondLimit {
                peer,
                rollback_point,
                max_point,
            } => write!(
                f,
                "RollbackBeyondLimit[{}, {}, {}]",
                peer, rollback_point, max_point
            ),
        }
    }
}

type State = (
    SelectChain,
    StageRef<ValidateHeaderEvent>,
    StageRef<ValidationFailed>,
);

#[instrument(
    level = Level::TRACE,
    skip_all,
    name = "stage.select_chain",
)]
pub fn stage(
    (mut select_chain, downstream, errors): State,
    msg: DecodedChainSyncEvent,
    eff: impl ConsensusOps + 'static,
) -> BoxFuture<'static, State> {
    Box::pin({
        let store = eff.store();
        async move {
            let peer = msg.peer();
            adopt_current_span(&msg);
            let events = match select_chain.handle_chain_sync(store, msg).await {
                Ok(events) => events,
                Err(e) => {
                    eff.base()
                        .send(&errors, ValidationFailed::new(&peer, e))
                        .await;
                    return (select_chain, downstream, errors);
                }
            };

            for event in events {
                eff.base().send(&downstream, event).await;
            }

            (select_chain, downstream, errors)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::effects::mock_consensus_ops;
    use crate::consensus::errors::ValidationFailed;
    use crate::consensus::events::DecodedChainSyncEvent;
    use crate::consensus::headers_tree::Tracker;
    use crate::consensus::headers_tree::Tracker::{Me, SomePeer};
    use crate::consensus::stages::select_chain::SyncTracker;
    use amaru_kernel::peer::Peer;
    use amaru_ouroboros_traits::fake::tests::{any_header, any_headers_chain, run};
    use pure_stage::StageRef;
    use std::collections::BTreeMap;
    use std::slice;
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
    async fn a_roll_forward_updates_the_tree_state() -> anyhow::Result<()> {
        let peer = Peer::new("name");
        let header = run(any_header());
        let message = make_roll_forward_message(&peer, &header);
        let consensus_ops = mock_consensus_ops();

        let (select_chain, _, _) =
            stage(make_state(&peer), message.clone(), consensus_ops.clone()).await;
        let output = make_validated_event(&peer, &header);

        assert_eq!(
            consensus_ops.mock_base.received(),
            BTreeMap::from_iter(vec![(
                "downstream".to_string(),
                vec![format!("{:?}", output)]
            )])
        );

        check_peers(
            select_chain,
            vec![
                (Me, vec![header.hash()]),
                (SomePeer(peer), vec![header.hash()]),
            ],
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_rollback_updates_the_tree_state() -> anyhow::Result<()> {
        let peer = Peer::new("name");
        let headers = run(any_headers_chain(2));
        let header1 = headers[0].clone();
        let header2 = headers[1].clone();

        let message1 = make_roll_forward_message(&peer, &header1);
        let message2 = make_roll_forward_message(&peer, &header2);
        let message3 = make_rollback_message(&peer, &header1);

        let consensus_ops = mock_consensus_ops();
        let state = stage(make_state(&peer), message1, consensus_ops.clone()).await;
        let state = stage(state, message2, consensus_ops.clone()).await;
        let (select_chain, _, _) = stage(state, message3.clone(), consensus_ops.clone()).await;

        let output1 = make_validated_event(&peer, &header1);
        let output2 = make_validated_event(&peer, &header2);

        assert_eq!(
            consensus_ops.mock_base.received(),
            BTreeMap::from_iter(vec![(
                "downstream".to_string(),
                vec![format!("{:?}", output1), format!("{:?}", output2)]
            )])
        );

        check_peers(
            select_chain,
            vec![
                (Me, vec![header1.hash(), header2.hash()]),
                (SomePeer(peer), vec![header1.hash()]),
            ],
        );
        Ok(())
    }

    // HELPERS

    fn make_state(peer: &Peer) -> State {
        let downstream: StageRef<ValidateHeaderEvent> = StageRef::named("downstream");
        let errors: StageRef<ValidationFailed> = StageRef::named("errors");
        (
            SelectChain::new(HeadersTreeState::new(10), slice::from_ref(peer)),
            downstream,
            errors,
        )
    }

    fn make_roll_forward_message(peer: &Peer, header: &Header) -> DecodedChainSyncEvent {
        DecodedChainSyncEvent::RollForward {
            peer: peer.clone(),
            point: header.point(),
            header: header.clone(),
            span: Span::current(),
        }
    }

    fn make_rollback_message(peer: &Peer, header: &Header) -> DecodedChainSyncEvent {
        DecodedChainSyncEvent::Rollback {
            peer: peer.clone(),
            rollback_point: header.point(),
            span: Span::current(),
        }
    }

    fn make_validated_event(peer: &Peer, header: &Header) -> ValidateHeaderEvent {
        ValidateHeaderEvent::Validated {
            peer: peer.clone(),
            header: header.clone(),
            span: Span::current(),
        }
    }

    fn check_peers(select_chain: SelectChain, expected: Vec<(Tracker, Vec<Hash<32>>)>) {
        let actual = select_chain
            .tree_state
            .peers()
            .iter()
            .map(|(k, vs)| (k.clone(), vs.clone()))
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }
}
