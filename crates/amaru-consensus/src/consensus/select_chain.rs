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

use super::{DecodedChainSyncEvent, ValidateHeaderEvent};
use crate::{
    ConsensusError,
    consensus::{EVENT_TARGET, ValidationFailed, headers_tree::HeadersTree},
    span::adopt_current_span,
};
use amaru_kernel::{HEADER_HASH_SIZE, Header, Point, peer::Peer, string_utils::ListToString};
use amaru_ouroboros::IsHeader;
use anyhow::anyhow;
use pallas_crypto::hash::Hash;
use pure_stage::{Effects, StageRef};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fmt::{Debug, Display, Formatter},
};
use tracing::{Level, Span, debug, info, instrument, trace, warn};

pub const DEFAULT_MAXIMUM_FRAGMENT_LENGTH: usize = 2160;

#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub struct SelectChain {
    chain_selector: HeadersTree<Header>,
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
        if !self.syncing_peers.remove(peer) {
            warn!("unknown caught-up peer {}", peer);
        }
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
    pub fn new(chain_selector: HeadersTree<Header>, peers: &[Peer]) -> Self {
        let sync_tracker = SyncTracker::new(peers);
        SelectChain {
            chain_selector,
            sync_tracker,
        }
    }

    fn forward_block(peer: Peer, header: Header, span: Span) -> ValidateHeaderEvent {
        ValidateHeaderEvent::Validated { peer, header, span }
    }

    fn switch_to_fork(
        peer: Peer,
        rollback_point: Point,
        fork: Vec<Header>,
        span: Span,
    ) -> Vec<ValidateHeaderEvent> {
        let mut result = vec![ValidateHeaderEvent::Rollback {
            rollback_point,
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
        peer: Peer,
        header: Header,
        span: Span,
    ) -> Result<Vec<ValidateHeaderEvent>, ConsensusError> {
        let result = self.chain_selector.select_roll_forward(&peer, header)?;

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
                rollback_point,
                fork,
            }) => {
                info!(target: EVENT_TARGET, rollback = %rollback_point, length = fork.len(), "switching to fork");
                SelectChain::switch_to_fork(peer, rollback_point, fork, span)
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
        peer: Peer,
        rollback_point: Point,
        span: Span,
    ) -> Result<Vec<ValidateHeaderEvent>, ConsensusError> {
        let result = self
            .chain_selector
            .select_rollback(&peer, &rollback_point.hash())?;

        match result {
            RollbackChainSelection::SwitchToFork(Fork {
                peer,
                rollback_point,
                fork,
            }) => {
                info!(target: EVENT_TARGET, rollback = %rollback_point, length = fork.len(), "switching to fork");
                Ok(SelectChain::switch_to_fork(
                    peer,
                    rollback_point,
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

    pub async fn handle_chain_sync(
        &mut self,
        chain_sync: DecodedChainSyncEvent,
    ) -> Result<Vec<ValidateHeaderEvent>, ConsensusError> {
        match chain_sync {
            DecodedChainSyncEvent::RollForward {
                peer, header, span, ..
            } => self.select_chain(peer, header, span).await,
            DecodedChainSyncEvent::Rollback {
                peer,
                rollback_point,
                span,
            } => self.select_rollback(peer, rollback_point, span).await,
            DecodedChainSyncEvent::CaughtUp { peer, .. } => self.caught_up(&peer),
        }
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
    pub rollback_point: Point,
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
                rollback_point,
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
                rollback_point,
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
    StageRef<StageError>,
);

#[instrument(
    level = Level::TRACE,
    skip_all,
    name = "stage.select_chain",
)]
pub async fn stage(
    (mut select_chain, downstream, errors): State,
    msg: DecodedChainSyncEvent,
    eff: Effects<DecodedChainSyncEvent>,
) -> State {
    adopt_current_span(&msg);
    let peer = msg.peer();

    let events = match select_chain.handle_chain_sync(msg).await {
        Ok(events) => events,
        Err(e) => {
            eff.send(
                &errors,
                StageError::new(anyhow!(ValidationFailed::new(peer, e))),
            )
            .await;
            return (select_chain, downstream, errors);
        }
    };

    for event in events {
        eff.send(&downstream, event).await;
    }

    (select_chain, downstream, errors)
}

#[cfg(test)]
mod tests {
    use amaru_kernel::peer::Peer;

    use crate::consensus::select_chain::SyncTracker;

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
