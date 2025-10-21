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

use crate::consensus::{
    EVENT_TARGET,
    effects::{BaseOps, ConsensusOps},
    errors::{ConsensusError, ValidationFailed},
    events::{BlockValidationResult, DecodedChainSyncEvent, ValidateHeaderEvent},
    headers_tree::{HeadersTree, HeadersTreeState},
    span::HasSpan,
};
use amaru_kernel::{HeaderHash, Point, peer::Peer, string_utils::ListToString};
use amaru_ouroboros::{BlockHeader, IsHeader};
use amaru_ouroboros_traits::ChainStore;
use pure_stage::{BoxFuture, StageRef};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::{
    fmt::{Debug, Display, Formatter},
    mem,
};
use tracing::{Level, Span, info, span, trace};

pub const DEFAULT_MAXIMUM_FRAGMENT_LENGTH: usize = 2160;

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct SelectChain {
    tree_state: HeadersTreeState,
}

impl SelectChain {
    pub fn new(tree_state: HeadersTreeState) -> Self {
        SelectChain { tree_state }
    }

    fn forward_block(peer: Peer, header: BlockHeader, span: Span) -> ValidateHeaderEvent {
        ValidateHeaderEvent::Validated { peer, header, span }
    }

    fn switch_to_fork(
        peer: Peer,
        rollback_point: Point,
        fork: Vec<BlockHeader>,
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
        store: Arc<dyn ChainStore<BlockHeader>>,
        peer: Peer,
        header: BlockHeader,
        span: Span,
    ) -> Result<Vec<ValidateHeaderEvent>, ConsensusError> {
        // Temporarily take the tree state out of self, to avoid borrowing self
        let tree_state = mem::take(&mut self.tree_state);
        let mut headers_tree = HeadersTree::create(store, tree_state);
        let result = headers_tree.select_roll_forward(&peer, &header)?;
        self.tree_state = headers_tree.into_tree_state();

        let events = match result {
            ForwardChainSelection::NewTip { peer, tip } => {
                vec![SelectChain::forward_block(peer, tip, span)]
            }
            ForwardChainSelection::SwitchToFork(Fork {
                peer,
                rollback_header,
                fork,
            }) => {
                info!(target: EVENT_TARGET, rollback = %rollback_header.point(), length = fork.len(), "switching to fork");
                SelectChain::switch_to_fork(peer, rollback_header.point(), fork, span)
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
        store: Arc<dyn ChainStore<BlockHeader>>,
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
                    rollback_header.point(),
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
        store: Arc<dyn ChainStore<BlockHeader>>,
        chain_sync: DecodedChainSyncEvent,
    ) -> BoxFuture<'_, Result<Vec<ValidateHeaderEvent>, ConsensusError>> {
        Box::pin(async move {
            match chain_sync {
                DecodedChainSyncEvent::RollForward {
                    peer, header, span, ..
                } => self.select_chain(store.clone(), peer, header, span).await,
                DecodedChainSyncEvent::Rollback {
                    peer,
                    rollback_point,
                    span,
                } => {
                    self.select_rollback(store.clone(), peer, rollback_point, span)
                        .await
                }
            }
        })
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
        rollback_point: HeaderHash,
        max_point: HeaderHash,
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
    StageRef<BlockValidationResult>,
    StageRef<ValidationFailed>,
);

pub async fn stage(
    (mut select_chain, downstream, errors): State,
    msg: DecodedChainSyncEvent,
    eff: impl ConsensusOps,
) -> State {
    let store = eff.store();
    let peer = msg.peer();
    let span = span!(parent: msg.span(), Level::TRACE, "stage.select_chain");
    let _entered = span.enter();

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
        match event {
            ValidateHeaderEvent::Validated { peer, header, span } => {
                let event = BlockValidationResult::BlockValidated { peer, header, span };
                eff.base().send(&downstream, event).await;
            }
            ValidateHeaderEvent::Rollback {
                peer,
                rollback_point,
                span,
            } => match eff.store().load_header(&rollback_point.hash()) {
                Some(rollback_header) => {
                    let event = BlockValidationResult::RolledBackTo {
                        peer,
                        rollback_header,
                        span,
                    };
                    eff.base().send(&downstream, event).await;
                }
                None => {
                    eff.base()
                        .send(
                            &errors,
                            ValidationFailed::new(
                                &peer,
                                ConsensusError::UnknownPoint(rollback_point.hash()),
                            ),
                        )
                        .await
                }
            },
        }
    }

    (select_chain, downstream, errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::headers_tree::Tracker::{Me, SomePeer};
    use crate::consensus::{
        effects::mock_consensus_ops,
        errors::ValidationFailed,
        events::{BlockValidationResult::BlockValidated, DecodedChainSyncEvent},
        headers_tree::Tracker,
    };
    use amaru_kernel::peer::Peer;
    use amaru_ouroboros_traits::tests::{any_header, any_headers_chain, run};
    use pure_stage::StageRef;
    use std::collections::BTreeMap;
    use tracing::Span;

    #[tokio::test]
    async fn a_roll_forward_updates_the_tree_state() -> anyhow::Result<()> {
        let peer = Peer::new("name");
        let header = run(any_header());
        let message = make_roll_forward_message(&peer, &header);
        let consensus_ops = mock_consensus_ops();
        consensus_ops.store().store_header(&header).unwrap();

        let store = consensus_ops.store();
        if let Some(parent_hash) = header.parent() {
            store.set_anchor_hash(&parent_hash)?;
        }
        let anchor = store.get_anchor_hash();
        let state = make_state(store.clone(), &peer, &anchor);
        let (select_chain, _, _) = stage(state, message.clone(), consensus_ops.clone()).await;
        let output = make_block_validated_event(&peer, &header);

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
                (Me, vec![anchor, header.hash()]),
                (SomePeer(peer), vec![anchor, header.hash()]),
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
        let store = consensus_ops.store();
        if let Some(parent_hash) = header1.parent() {
            store.set_anchor_hash(&parent_hash)?;
        }

        store.store_header(&header1).unwrap();
        store.store_header(&header2).unwrap();

        let anchor = store.get_anchor_hash();
        let state = make_state(consensus_ops.store(), &peer, &anchor);
        let state = stage(state, message1, consensus_ops.clone()).await;
        let state = stage(state, message2, consensus_ops.clone()).await;
        let (select_chain, _, _) = stage(state, message3.clone(), consensus_ops.clone()).await;

        let output1 = make_block_validated_event(&peer, &header1);
        let output2 = make_block_validated_event(&peer, &header2);

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
                (Me, vec![anchor, header1.hash(), header2.hash()]),
                (SomePeer(peer), vec![anchor, header1.hash()]),
            ],
        );
        Ok(())
    }

    // HELPERS

    fn make_state(
        store: Arc<dyn ChainStore<BlockHeader>>,
        peer: &Peer,
        anchor: &HeaderHash,
    ) -> State {
        let downstream: StageRef<BlockValidationResult> = StageRef::named("downstream");
        let errors: StageRef<ValidationFailed> = StageRef::named("errors");
        let mut tree_state = HeadersTreeState::new(10);
        tree_state
            .initialize_peer(store.clone(), peer, anchor)
            .unwrap();
        (SelectChain::new(tree_state), downstream, errors)
    }

    fn make_roll_forward_message(peer: &Peer, header: &BlockHeader) -> DecodedChainSyncEvent {
        DecodedChainSyncEvent::RollForward {
            peer: peer.clone(),
            header: header.clone(),
            span: Span::current(),
        }
    }

    fn make_rollback_message(peer: &Peer, header: &BlockHeader) -> DecodedChainSyncEvent {
        DecodedChainSyncEvent::Rollback {
            peer: peer.clone(),
            rollback_point: header.point(),
            span: Span::current(),
        }
    }

    fn make_block_validated_event(peer: &Peer, header: &BlockHeader) -> BlockValidationResult {
        BlockValidated {
            peer: peer.clone(),
            header: header.clone(),
            span: Span::current(),
        }
    }

    fn check_peers(select_chain: SelectChain, expected: Vec<(Tracker, Vec<HeaderHash>)>) {
        let actual = select_chain
            .tree_state
            .peers()
            .iter()
            .map(|(k, vs)| (k.clone(), vs.clone()))
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }
}
