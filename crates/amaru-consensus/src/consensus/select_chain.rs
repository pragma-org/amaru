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
use crate::consensus::ValidationFailed;
use crate::consensus::headers_tree::HeadersTree;
use crate::span::adopt_current_span;
use crate::{ConsensusError, consensus::EVENT_TARGET};
use amaru_kernel::{Header, Point, peer::Peer};
use amaru_ouroboros::IsHeader;
use pallas_crypto::hash::Hash;
use pure_stage::{Effects, StageRef, Void};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{Level, Span, debug, instrument, trace};

pub const DEFAULT_MAXIMUM_FRAGMENT_LENGTH: usize = 2160;

#[derive(Debug, Deserialize, Serialize)]
pub struct SelectChain {
    #[serde(skip, default = "default_chain_selector")]
    chain_selector: Arc<Mutex<HeadersTree<Header>>>,
    sync_tracker: SyncTracker,
    is_caught_up: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SyncTracker {
    peers_state: BTreeMap<Peer, SyncState>,
}

impl SyncTracker {
    pub fn new(peers: &[&Peer]) -> Self {
        let peers_state = peers
            .iter()
            .map(|p| ((*p).clone(), SyncState::Syncing))
            .collect();
        Self { peers_state }
    }

    pub fn caught_up(&mut self, peer: &Peer) {
        if let Some(s) = self.peers_state.get_mut(peer) {
            *s = SyncState::CaughtUp;
        }
    }

    pub fn is_caught_up(&self) -> bool {
        self.peers_state.values().all(|s| *s == SyncState::CaughtUp)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub enum SyncState {
    Syncing,
    CaughtUp,
}

/// FIXME: a default chain selector will not work as it is since a headers tree needs to be
/// initialized before being used.
fn default_chain_selector() -> Arc<Mutex<HeadersTree<Header>>> {
    Arc::new(Mutex::new(HeadersTree::new(
        DEFAULT_MAXIMUM_FRAGMENT_LENGTH,
        None,
    )))
}

impl PartialEq for SelectChain {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl SelectChain {
    pub fn new(chain_selector: Arc<Mutex<HeadersTree<Header>>>, peers: &Vec<&Peer>) -> Self {
        let sync_tracker = SyncTracker::new(peers);
        SelectChain {
            chain_selector,
            sync_tracker,
            is_caught_up: false,
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
        let result = self
            .chain_selector
            .lock()
            .await
            .select_roll_forward(&peer, header)?;

        let events = match result {
            ForwardChainSelection::NewTip { peer, tip } => {
                debug!(target: EVENT_TARGET, hash = %tip.hash(), slot = %tip.slot(), "new_tip");

                vec![SelectChain::forward_block(peer, tip, span)]
            }
            ForwardChainSelection::SwitchToFork(Fork {
                peer,
                rollback_point,
                fork,
            }) => {
                debug!(target: EVENT_TARGET, rollback = %rollback_point, length = fork.len(), "switching to fork");

                SelectChain::switch_to_fork(peer, rollback_point, fork, span)
            }
            ForwardChainSelection::NoChange => {
                trace!(target: EVENT_TARGET, "no_change");
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
            .lock()
            .await
            .select_rollback(&peer, &rollback_point.hash())?;

        match result {
            RollbackChainSelection::RollbackTo(hash) => {
                debug!(target: EVENT_TARGET, %hash, "rollback");
                Ok(vec![ValidateHeaderEvent::Rollback {
                    rollback_point,
                    peer,
                    span,
                }])
            }
            RollbackChainSelection::SwitchToFork(Fork {
                peer,
                rollback_point,
                fork,
            }) => {
                debug!(target: EVENT_TARGET, rollback = %rollback_point, length = fork.len(), "switching to fork");

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
        self.is_caught_up = self.sync_tracker.is_caught_up();

        Ok(vec![])
    }
}

/// Definition of a fork.
///
/// FIXME: The peer should not be needed here, as the fork should be
/// comprised of known blocks. It is only needed to download the blocks
/// we don't currently store.
#[derive(Debug, PartialEq)]
pub struct Fork<H: IsHeader> {
    pub peer: Peer,
    pub rollback_point: Point,
    pub fork: Vec<H>,
}

/// The outcome of the chain selection process in  case of
/// roll forward.
#[derive(Debug, PartialEq)]
pub enum ForwardChainSelection<H: IsHeader> {
    /// The current best chain has been extended with a (single) new header.
    NewTip { peer: Peer, tip: H },

    /// The current best chain is unchanged.
    NoChange,

    /// The current best chain has switched to given fork.
    SwitchToFork(Fork<H>),
}

/// The outcome of the chain selection process in case of rollback
#[derive(Debug, PartialEq)]
pub enum RollbackChainSelection<H: IsHeader> {
    /// The current best chain has been rolled back to the given hash.
    RollbackTo(Hash<32>),

    /// The current best chain has switched to given fork.
    SwitchToFork(Fork<H>),

    /// The peer tried to rollback beyond the limit
    RollbackBeyondLimit {
        peer: Peer,
        rollback_point: Hash<32>,
        max_point: Hash<32>,
    },

    /// The current best chain as not changed
    NoChange,
}

type State = (
    SelectChain,
    StageRef<ValidateHeaderEvent, Void>,
    StageRef<ValidationFailed, Void>,
);

#[instrument(
    level = Level::TRACE,
    skip_all,
    name = "stage.select_chain",
)]
pub async fn stage(
    (mut select_chain, downstream, errors): State,
    msg: DecodedChainSyncEvent,
    eff: Effects<DecodedChainSyncEvent, State>,
) -> State {
    adopt_current_span(&msg);
    let peer = msg.peer();

    let point = msg.point();
    let events = match select_chain.handle_chain_sync(msg).await {
        Ok(events) => events,
        Err(e) => {
            eff.send(&errors, ValidationFailed::new(peer, point, e))
                .await;
            return (select_chain, downstream, errors);
        }
    };

    if events.is_empty() {
        tracing::info!(%peer, %point, "no events to send");
    }
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
        let peers = vec![&alice, &bob];
        let mut tracker = SyncTracker::new(&peers);

        tracker.caught_up(&alice);
        tracker.caught_up(&bob);

        assert!(tracker.is_caught_up());
    }

    #[tokio::test]
    async fn is_not_caught_up_given_some_peer_is_not() {
        let alice = Peer::new("alice");
        let bob = Peer::new("bob");
        let peers = vec![&alice, &bob];
        let mut tracker = SyncTracker::new(&peers);

        tracker.caught_up(&alice);

        assert!(!tracker.is_caught_up());
    }
}
