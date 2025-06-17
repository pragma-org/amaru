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

use super::{chain_selection::RollbackChainSelection, DecodedChainSyncEvent, ValidateHeaderEvent};
use crate::{
    consensus::{
        chain_selection::{self, ChainSelector, Fork},
        EVENT_TARGET,
    },
    peer::Peer,
    ConsensusError,
};
use amaru_kernel::{Hash, Header, Point};
use amaru_ouroboros::IsHeader;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{trace, Span};

pub struct SelectChain {
    chain_selector: Arc<Mutex<ChainSelector<Header>>>,
}

impl SelectChain {
    pub fn new(chain_selector: Arc<Mutex<ChainSelector<Header>>>) -> Self {
        SelectChain { chain_selector }
    }

    fn forward_block<H: IsHeader>(&self, peer: Peer, header: H, span: Span) -> ValidateHeaderEvent {
        ValidateHeaderEvent::Validated {
            peer,
            point: header.point(),
            span,
        }
    }

    fn switch_to_fork(
        &self,
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
            result.push(self.forward_block(peer.clone(), header, span.clone()));
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
            .select_roll_forward(&peer, header);

        let events = match result {
            chain_selection::ForwardChainSelection::NewTip(hdr) => {
                trace!(target: EVENT_TARGET, hash = %hdr.hash(), "new_tip");
                vec![self.forward_block(peer, hdr, span)]
            }
            chain_selection::ForwardChainSelection::SwitchToFork(Fork {
                peer,
                rollback_point,
                tip: _,
                fork,
            }) => {
                trace!(target: EVENT_TARGET, rollback = %rollback_point, "switching to fork");
                self.switch_to_fork(peer, rollback_point, fork, span)
            }
            chain_selection::ForwardChainSelection::NoChange => {
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
            .select_rollback(&peer, Hash::from(&rollback_point));

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
                tip: _,
            }) => Ok(self.switch_to_fork(peer, rollback_point, fork, span)),
            RollbackChainSelection::NoChange => Ok(vec![]),
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
