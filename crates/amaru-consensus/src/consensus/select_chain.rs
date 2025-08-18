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

use std::fmt::{Display, Formatter};
use super::{DecodedChainSyncEvent, ValidateHeaderEvent};
use crate::consensus::headers_tree::HeadersTree;
use crate::{consensus::EVENT_TARGET, ConsensusError};
use amaru_kernel::{peer::Peer, Header, Point};
use amaru_ouroboros::IsHeader;
use pallas_crypto::hash::Hash;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{trace, Span};
use crate::consensus::select_chain::RollbackChainSelection::RollbackTo;

pub const DEFAULT_MAXIMUM_FRAGMENT_LENGTH: usize = 2160;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SelectChain {
    #[serde(skip, default = "default_chain_selector")]
    chain_selector: Arc<Mutex<HeadersTree<Header>>>,
}

/// FIXME: a default chain selector will not work as it is since a headers tree needs to be
/// initialized before being used.
fn default_chain_selector() -> Arc<Mutex<HeadersTree<Header>>> {
    Arc::new(Mutex::new(HeadersTree::new(
        DEFAULT_MAXIMUM_FRAGMENT_LENGTH,
        &None,
    )))
}

impl PartialEq for SelectChain {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl SelectChain {
    pub fn new(chain_selector: Arc<Mutex<HeadersTree<Header>>>) -> Self {
        SelectChain { chain_selector }
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
                trace!(target: EVENT_TARGET, hash = %tip.hash(), "new_tip");
                vec![SelectChain::forward_block(peer, tip, span)]
            }
            ForwardChainSelection::SwitchToFork(Fork {
                                                    peer,
                                                    rollback_point,
                                                    fork,
                                                }) => {
                trace!(target: EVENT_TARGET, rollback = %rollback_point, "switching to fork");
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
                trace!(target: EVENT_TARGET, %hash, "rollback");
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
                                                 }) => Ok(SelectChain::switch_to_fork(
                peer,
                rollback_point,
                fork,
                span,
            )),
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
        }
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
                f.write_str(&format!("NewTip[{}, {}]", peer, tip))
            }
            ForwardChainSelection::NoChange => {
                f.write_str("NoChange")
            }
            ForwardChainSelection::SwitchToFork(Fork {
                                                    peer, rollback_point, fork
                                                }) => {
                f.write_str(&format!("SwitchToFork[{}, {}, {}]", peer, rollback_point, fork.iter().map(|h| h.to_string()).collect::<Vec<_>>().join(", ")))
            }
        }
    }
}

/// The outcome of the chain selection process in case of rollback
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
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

impl<H: IsHeader + Display> Display for RollbackChainSelection<H> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            RollbackTo(hash) => {
                f.write_str(&format!("RollbackTo[{}]", hash))
            }
            RollbackChainSelection::NoChange => {
                f.write_str("NoChange")
            }
            RollbackChainSelection::SwitchToFork(Fork {
                                                     peer, rollback_point, fork
                                                 }) => {
                f.write_str(&format!("SwitchToFork[{}, {}, {}]", peer, rollback_point, fork.iter().map(|h| h.to_string()).collect::<Vec<_>>().join(", ")))
            }

            RollbackChainSelection::RollbackBeyondLimit { peer, rollback_point, max_point } => {
                f.write_str(&format!("RollbackBeyondLimit[{}, {}, {}]", peer, rollback_point, max_point))
            }
        }
    }
}
