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

use crate::{
    effects::{BaseOps, ConsensusOps},
    errors::{ConsensusError, ValidationFailed},
    events::{BlockValidationResult, DecodedChainSyncEvent, ValidateHeaderEvent},
    headers_tree::{HeadersTree, HeadersTreeState},
    span::HasSpan,
};
use amaru_kernel::{BlockHeader, HeaderHash, IsHeader, Peer, Point, utils::string::ListToString};
use amaru_observability::amaru::consensus::chain_sync::SELECT_CHAIN;
use amaru_ouroboros_traits::ChainStore;
use pure_stage::{BoxFuture, StageRef};
use std::{
    fmt::{Debug, Display, Formatter},
    mem,
    sync::Arc,
};
use tracing::{Instrument, Span, debug, info, trace};

pub const DEFAULT_MAXIMUM_FRAGMENT_LENGTH: usize = 2160;

#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
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

    pub async fn select_roll_forward(
        &mut self,
        store: Arc<dyn ChainStore<BlockHeader>>,
        peer: Peer,
        header: BlockHeader,
        span: Span,
    ) -> Result<Vec<ValidateHeaderEvent>, ConsensusError> {
        // Temporarily take the tree state out of self, to avoid borrowing self
        let tree_state = mem::take(&mut self.tree_state);
        let mut headers_tree = HeadersTree::create(store, tree_state);
        let result = headers_tree.select_roll_forward(&peer, &header);
        // Make sure to put back the tree state before raising any error
        self.tree_state = headers_tree.into_tree_state();

        let events = match result? {
            ForwardChainSelection::NewTip { peer, tip } => {
                vec![SelectChain::forward_block(peer, tip, span)]
            }
            ForwardChainSelection::SwitchToFork(Fork {
                peer,
                rollback_header,
                fork,
            }) => {
                debug!(rollback_point = %rollback_header.point(), length = fork.len(), "roll_forward.switch_to_fork");
                SelectChain::switch_to_fork(peer, rollback_header.point(), fork, span)
            }
            ForwardChainSelection::NoChange => {
                trace!("roll_forward.no_change");
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
                info!(rollback_point = %rollback_header.point(), length = fork.len(), "rollback.switch_to_fork");
                Ok(SelectChain::switch_to_fork(
                    peer,
                    rollback_header.point(),
                    fork,
                    span,
                ))
            }
            RollbackChainSelection::NoChange => {
                trace!("rollback.no_change");
                Ok(vec![])
            }
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
                } => {
                    self.select_roll_forward(store.clone(), peer, header, span)
                        .await
                }
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
/// TODO: The peer should not be needed here, as the fork should be
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

pub fn stage(
    (mut select_chain, downstream, errors): State,
    msg: DecodedChainSyncEvent,
    eff: impl ConsensusOps,
) -> impl Future<Output = State> {
    let span = tracing::trace_span!(parent: msg.span(), SELECT_CHAIN);
    async move {
        let store = eff.store();
        let peer = msg.peer();
        let events = match select_chain.handle_chain_sync(store.clone(), msg).await {
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
                    match store.roll_forward_chain(&header.point()) {
                        Ok(()) => {
                            let event =
                                BlockValidationResult::BlockValidated { peer, header, span };
                            eff.base().send(&downstream, event).await;
                        }
                        Err(e) => {
                            eff.base()
                                .send(
                                    &errors,
                                    ValidationFailed {
                                        peer,
                                        error: ConsensusError::RollForwardChainFailed(
                                            header.hash(),
                                            e,
                                        ),
                                    },
                                )
                                .await
                        }
                    }
                }
                ValidateHeaderEvent::Rollback {
                    peer,
                    rollback_point,
                    span,
                } => match eff.store().load_header(&rollback_point.hash()) {
                    Some(rollback_header) => match store.rollback_chain(&rollback_header.point()) {
                        Ok(_size) => {
                            let event = BlockValidationResult::RolledBackTo {
                                peer,
                                rollback_header,
                                span,
                            };
                            eff.base().send(&downstream, event).await;
                        }
                        Err(e) => {
                            eff.base()
                                .send(
                                    &errors,
                                    ValidationFailed {
                                        peer,
                                        error: ConsensusError::RollbackChainFailed(
                                            rollback_header.point(),
                                            e,
                                        ),
                                    },
                                )
                                .await
                        }
                    },
                    None => {
                        let err = ConsensusError::UnknownPoint(rollback_point.hash());
                        let msg = ValidationFailed::new(&peer, err);
                        eff.base().send(&errors, msg).await
                    }
                },
            }
        }

        (select_chain, downstream, errors)
    }
    .instrument(span)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        effects::mock_consensus_ops,
        errors::{InvalidHeaderParentData, ValidationFailed},
        headers_tree::{
            Tracker,
            Tracker::{Me, SomePeer},
        },
    };
    use amaru_kernel::{Peer, any_headers_chain, utils::tests::run_strategy};
    use pure_stage::StageRef;
    use std::collections::BTreeMap;
    use tracing::Span;

    #[tokio::test]
    async fn a_roll_forward_updates_the_tree_state() -> anyhow::Result<()> {
        let peer = Peer::new("name");
        let headers = run_strategy(any_headers_chain(2));
        let header0 = headers[0].clone();
        let header1 = headers[1].clone();

        let consensus_ops = mock_consensus_ops();
        let store = consensus_ops.store();
        store.store_header(&header0)?;
        store.store_header(&header1)?;
        store.set_anchor_hash(&header0.hash())?;
        store.set_best_chain_hash(&header0.hash())?;

        let state = make_state(store.clone(), &[&peer], &header0.hash());
        let message = make_roll_forward_message(&peer, &header1);
        let (select_chain, _, _) = stage(state, message.clone(), consensus_ops.clone()).await;
        let output = make_block_validated_event(&peer, &header1);

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
                (Me, vec![header0.hash(), header1.hash()]),
                (SomePeer(peer), vec![header0.hash(), header1.hash()]),
            ],
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_rollback_updates_the_tree_state() -> anyhow::Result<()> {
        let peer = Peer::new("name");
        let headers = run_strategy(any_headers_chain(3));
        let header0 = headers[0].clone();
        let header1 = headers[1].clone();
        let header2 = headers[2].clone();

        let consensus_ops = mock_consensus_ops();
        let store = consensus_ops.store();
        store.store_header(&header0)?;
        store.store_header(&header1)?;
        store.store_header(&header2)?;
        store.set_anchor_hash(&header0.hash())?;
        store.set_best_chain_hash(&header0.hash())?;

        let state = make_state(store, &[&peer], &header0.hash());
        let message1 = make_roll_forward_message(&peer, &header1);
        let message2 = make_roll_forward_message(&peer, &header2);
        let message3 = make_rollback_message(&peer, &header1);

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
                (Me, vec![header0.hash(), header1.hash(), header2.hash()]),
                (SomePeer(peer), vec![header0.hash(), header1.hash()]),
            ],
        );
        Ok(())
    }

    /// This test reproduces a scenario where a peer sends headers in an incorrect order (due
    /// to network latencies for example). This causes an error to be raised when processing an
    /// event. That error should not reset the tree state.
    #[tokio::test]
    async fn an_error_does_not_reset_the_tree_state() -> anyhow::Result<()> {
        let peer1 = Peer::new("peer1");
        let peer2 = Peer::new("peer2");
        let headers = run_strategy(any_headers_chain(4));
        let header0 = headers[0].clone();
        let header1 = headers[1].clone();
        let header2 = headers[2].clone();
        let header3 = headers[3].clone();

        let consensus_ops = mock_consensus_ops();
        let store = consensus_ops.store();
        store.store_header(&header0)?;
        store.store_header(&header1)?;
        store.store_header(&header2)?;
        store.store_header(&header3)?;
        store.set_anchor_hash(&header0.hash())?;
        store.set_best_chain_hash(&header0.hash())?;

        let state = make_state(store.clone(), &[&peer1, &peer2], &header0.hash());

        let message1 = make_roll_forward_message(&peer1, &header1);
        let message2 = make_roll_forward_message(&peer2, &header1);
        let message3 = make_roll_forward_message(&peer2, &header3);
        let message4 = make_roll_forward_message(&peer1, &header2);

        let state1 = stage(state, message1, consensus_ops.clone()).await;
        let state2 = stage(state1, message2, consensus_ops.clone()).await;
        // message 3 will cause an error because header3's parent is header2, which
        // has not been sent yet by peer2
        let state3 = stage(state2.clone(), message3, consensus_ops.clone()).await;
        assert_eq!(
            state3.0.tree_state, state2.0.tree_state,
            "the tree state should not change after an error"
        );

        let (select_chain, _, _) = stage(state3, message4.clone(), consensus_ops.clone()).await;

        let output1 = make_block_validated_event(&peer1, &header1);
        let output2 = make_block_validated_event(&peer1, &header2);
        let failure = ValidationFailed::new(
            &peer2,
            ConsensusError::InvalidHeaderParent(Box::new(InvalidHeaderParentData {
                peer: peer2.clone(),
                forwarded: header3.point(),
                actual: Some(header2.hash()),
                expected: header1.point(),
            })),
        );

        assert_eq!(
            consensus_ops.mock_base.received(),
            BTreeMap::from_iter(vec![
                (
                    "downstream".to_string(),
                    vec![format!("{:?}", output1), format!("{:?}", output2)]
                ),
                ("errors".to_string(), vec![format!("{:?}", failure)])
            ])
        );

        check_peers(
            select_chain,
            vec![
                (Me, vec![header0.hash(), header1.hash(), header2.hash()]),
                (
                    SomePeer(peer1),
                    vec![header0.hash(), header1.hash(), header2.hash()],
                ),
                (SomePeer(peer2), vec![header0.hash(), header1.hash()]),
            ],
        );
        Ok(())
    }

    // HELPERS

    fn make_state(
        store: Arc<dyn ChainStore<BlockHeader>>,
        peers: &[&Peer],
        anchor: &HeaderHash,
    ) -> State {
        let downstream: StageRef<BlockValidationResult> = StageRef::named_for_tests("downstream");
        let errors: StageRef<ValidationFailed> = StageRef::named_for_tests("errors");
        let mut tree_state = HeadersTreeState::new(10);
        for peer in peers {
            tree_state
                .initialize_peer(store.clone(), peer, anchor)
                .unwrap();
        }
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
        BlockValidationResult::BlockValidated {
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
