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
use tracing::{Level, Span, instrument, trace};

pub const DEFAULT_MAXIMUM_FRAGMENT_LENGTH: usize = 2160;

#[derive(Debug, Deserialize, Serialize, PartialEq)]
pub struct SelectChain {
    chain_selector: HeadersTree<Header>,
}

impl SelectChain {
    pub fn new(chain_selector: HeadersTree<Header>) -> Self {
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
        let result = self.chain_selector.select_roll_forward(&peer, header)?;

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
            DecodedChainSyncEvent::CaughtUp { .. } => Ok(vec![]),
        }
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
