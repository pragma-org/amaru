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

use crate::store_effects::Store;
use crate::{
    blockfetch::{State, messages::Message},
    mux::MuxMessage,
    protocol::{
        Inputs, Miniprotocol, Outcome, PROTO_N2N_BLOCK_FETCH, ProtocolState, Responder, StageState,
        miniprotocol, outcome,
    },
};
use amaru_kernel::{BlockHeader, Point, RawBlock};
use amaru_ouroboros_traits::{ChainStore, ReadOnlyChainStore, StoreError};
use pure_stage::{DeserializerGuards, Effects, StageRef, Void};

pub fn register_deserializers() -> DeserializerGuards {
    vec![pure_stage::register_data_deserializer::<BlockFetchResponder>().boxed()]
}

pub fn responder() -> Miniprotocol<State, BlockFetchResponder, Responder> {
    miniprotocol(PROTO_N2N_BLOCK_FETCH.responder())
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BlockFetchResponder {
    muxer: StageRef<MuxMessage>,
}

/// This data type represents a range of points to fetch blocks for.
/// It can represent either a range along the best chain (from `from` to `through` inclusive),
/// or a list of points representing a fork. The main difference is that the points in a fork cannot
/// go beyond the current best chain anchor.
#[derive(Debug, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
pub enum PointsRange {
    BestChain { from: Point, through: Point },
    Fork(Vec<Point>),
    Empty,
}

impl PointsRange {
    /// Load the first available block in the current range, if any.
    /// The block is expected to be found since we must be using a valid range.
    /// Each time we attempt to fetch a block we pop its point from the current_range.
    #[expect(clippy::panic)]
    async fn next_block<T>(mut self, effects: Effects<T>) -> Option<(RawBlock, PointsRange)> {
        let store = Store::new(effects);
        match self {
            PointsRange::BestChain { from, through } => match store.load_block(&from.hash()) {
                Ok(block) => {
                    if from == through {
                        self = PointsRange::Empty;
                    } else if let Some(next) = store.next_best_chain(&from) {
                        self = PointsRange::BestChain {
                            from: next,
                            through,
                        }
                    } else {
                        return None;
                    }
                    Some((block, self))
                }
                Err(StoreError::NotFound { .. }) => None,
                Err(other) => panic!("{other:?}"),
            },
            PointsRange::Fork(points) => {
                if let Some((from, rest)) = points.split_first() {
                    match store.load_block(&from.hash()) {
                        Ok(block) => {
                            if rest.is_empty() {
                                self = PointsRange::Empty;
                            } else {
                                self = PointsRange::Fork(rest.to_vec());
                            }
                            Some((block, self))
                        }
                        Err(StoreError::NotFound { .. }) => None,
                        Err(other) => panic!("{other:?}"),
                    }
                } else {
                    None
                }
            }
            PointsRange::Empty => None,
        }
    }

    /// Return a points range:
    ///  - A BestChain range if both `from` and `through` are on the best chain and from <= through.
    ///  - A Fork range `from` and `through` exist in the store and anchor <= from <= through.
    pub fn request_range(
        store: &dyn ChainStore<BlockHeader>,
        from: Point,
        through: Point,
    ) -> Option<PointsRange> {
        // Check if both 'from' and 'through' points are on the best chain.
        if store.load_from_best_chain(&through).is_some() {
            if store.load_from_best_chain(&from).is_some() {
                // make sure that from <= through
                if from > through {
                    tracing::debug!(%from, %through, "requested range is invalid: from > through");
                    None
                } else {
                    let range = PointsRange::BestChain { from, through };
                    Some(range)
                }
            } else {
                tracing::debug!(%from, %through, "both points must be on the best chain");
                None
            }
        } else {
            // Otherwise the range must on a fork
            tracing::debug!(%from, "requested 'from' point is not on the best chain");

            if let Some(points) = store.ancestors_points(&from, &through) {
                Some(PointsRange::Fork(points))
            } else {
                tracing::debug!(%from, %through, "no common ancestor found in the requested range");
                None
            }
        }
    }
}

impl BlockFetchResponder {
    pub fn new(muxer: StageRef<MuxMessage>) -> (State, Self) {
        (State::Idle, Self { muxer })
    }
}

/// Local message for streaming blocks.
/// It is either done or it contains a range of points to fetch the next block from.
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum BlockStreaming {
    NextBlock(PointsRange),
    Done,
}

impl StageState<State, Responder> for BlockFetchResponder {
    type LocalIn = BlockStreaming;

    async fn local(
        self,
        _proto: &State,
        input: Self::LocalIn,
        eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<ResponderAction>, Self)> {
        match input {
            BlockStreaming::NextBlock(points_range) => {
                // Load the next block and:
                //  - Return it if found to the protocol stage .
                //  - Iterate with a local message containing the updated points range.
                if let Some((block, points_range)) = points_range.next_block(eff.clone()).await {
                    eff.send(
                        eff.me_ref(),
                        Inputs::Local(BlockStreaming::NextBlock(points_range)),
                    )
                    .await;
                    Ok((Some(ResponderAction::Block(block.to_vec())), self))
                } else {
                    // No more blocks to send, finish the batch.
                    eff.send(eff.me_ref(), Inputs::Local(BlockStreaming::Done))
                        .await;
                    Ok((None, self))
                }
            }
            BlockStreaming::Done => Ok((Some(ResponderAction::BatchDone), self)),
        }
    }

    async fn network(
        self,
        _proto: &State,
        input: ResponderResult,
        eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<ResponderAction>, Self)> {
        match input {
            ResponderResult::RequestRange { from, through } => {
                let store = Store::new(eff.clone());
                if let Some(points_range) = PointsRange::request_range(&store, from, through) {
                    eff.send(
                        eff.me_ref(),
                        Inputs::Local(BlockStreaming::NextBlock(points_range)),
                    )
                    .await;
                    Ok((Some(ResponderAction::StartBatch), self))
                } else {
                    Ok((Some(ResponderAction::NoBlocks), self))
                }
            }
            ResponderResult::Done => Ok((None, self)),
        }
    }

    fn muxer(&self) -> &StageRef<MuxMessage> {
        &self.muxer
    }
}

impl ProtocolState<Responder> for State {
    type WireMsg = Message;
    type Action = ResponderAction;
    type Out = ResponderResult;
    type Error = Void;

    fn init(&self) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out, Self::Error>, Self)> {
        Ok((outcome().want_next(), *self))
    }

    fn network(
        &self,
        input: Self::WireMsg,
    ) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out, Self::Error>, Self)> {
        use Message::*;
        match (self, input) {
            (Self::Idle, RequestRange { from, through }) => Ok((
                outcome().result(ResponderResult::RequestRange { from, through }),
                Self::Busy,
            )),
            (Self::Idle, ClientDone) => Ok((
                outcome().want_next().result(ResponderResult::Done),
                Self::Done,
            )),
            (state, msg) => anyhow::bail!("unexpected message in state {:?}: {:?}", state, msg),
        }
    }

    fn local(
        &self,
        input: Self::Action,
    ) -> anyhow::Result<(Outcome<Self::WireMsg, Void, Self::Error>, Self)> {
        use ResponderAction::*;
        match (self, input) {
            (Self::Busy, StartBatch) => Ok((outcome().send(Message::StartBatch), Self::Streaming)),
            (Self::Busy, NoBlocks) => {
                Ok((outcome().send(Message::NoBlocks).want_next(), Self::Idle))
            }
            (Self::Streaming, Block(body)) => {
                Ok((outcome().send(Message::Block { body }), Self::Streaming))
            }
            (Self::Streaming, BatchDone) => {
                Ok((outcome().send(Message::BatchDone).want_next(), Self::Idle))
            }
            (state, action) => {
                anyhow::bail!("unexpected action in state {:?}: {:?}", state, action)
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
pub enum ResponderAction {
    StartBatch,
    NoBlocks,
    Block(Vec<u8>),
    BatchDone,
}

#[derive(Debug, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
pub enum ResponderResult {
    RequestRange { from: Point, through: Point },
    Done,
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::protocol::Responder;
    use amaru_kernel::is_header::tests::{any_header_with_parent, any_headers_chain, run};
    use amaru_kernel::tests::random_hash;
    use amaru_kernel::{BlockHeader, IsHeader};
    use amaru_ouroboros_traits::ChainStore;
    use amaru_ouroboros_traits::in_memory_consensus_store::InMemConsensusStore;
    use std::sync::Arc;

    #[test]
    #[expect(clippy::wildcard_enum_match_arm)]
    fn test_responder_protocol() {
        crate::blockfetch::spec::<Responder>().check(State::Idle, |msg| match msg {
            Message::NoBlocks => Some(ResponderAction::NoBlocks),
            Message::StartBatch => Some(ResponderAction::StartBatch),
            Message::Block { body } => Some(ResponderAction::Block(body.clone())),
            Message::BatchDone => Some(ResponderAction::BatchDone),
            _ => None,
        });
    }

    #[test]
    fn request_range_best_chain_valid() {
        let (store, headers) = make_store_with_chain(5);
        let from = headers[1].point();
        let through = headers[3].point();
        let range = PointsRange::request_range(&*store, from, through);
        assert_eq!(range, Some(PointsRange::BestChain { from, through }));
    }

    #[test]
    fn request_range_best_chain_invalid_order() {
        let (store, headers) = make_store_with_chain(4);
        let from = headers[3].point();
        let through = headers[1].point();
        let range = PointsRange::request_range(&*store, from, through);
        assert_eq!(range, None);
    }

    #[test]
    fn request_range_from_not_on_best_chain() {
        let (store, headers) = make_store_with_chain(6);
        // a header not on the best chain
        let foreign = run(any_header_with_parent(headers[2].hash()));
        store.store_header(&foreign).unwrap();
        let from = foreign.point();
        let through = headers[3].point();
        let range = PointsRange::request_range(&*store, from, through);
        assert_eq!(range, None);
    }

    #[test]
    fn request_range_fork_success() {
        let (store, headers) = make_store_with_chain(6);
        let from = headers[2].point();
        // Create a fork from 'from'
        let via = run(any_header_with_parent(from.hash()));
        let through = run(any_header_with_parent(via.hash()));
        store.store_header(&via).unwrap();
        store.store_header(&through).unwrap();
        let range = PointsRange::request_range(&*store, from, through.point());
        assert_eq!(
            range,
            Some(PointsRange::Fork(vec![through.point(), via.point(), from]))
        );
    }

    #[test]
    fn request_range_fork_no_common_ancestor() {
        let (store, headers) = make_store_with_chain(5);
        // a header whose parent is not in the store
        let foreign = run(any_header_with_parent(random_hash()));
        let from = headers[1].point();
        let through = foreign.point();
        let range = PointsRange::request_range(&*store, from, through);
        assert_eq!(range, None);
    }

    #[test]
    fn request_range_fork_too_old() {
        let (store, headers) = make_store_with_chain(6);
        let from = headers[2].point();
        // Create a fork from 'from' but beyond the anchor
        let via = run(any_header_with_parent(from.hash()));
        let through = run(any_header_with_parent(via.hash()));
        store.store_header(&via).unwrap();
        store.set_anchor_hash(&via.hash()).unwrap();
        store.store_header(&through).unwrap();
        let range = PointsRange::request_range(&*store, from, through.point());
        assert_eq!(range, None);
    }

    // HELPERS

    fn make_store_with_chain(
        n: usize,
    ) -> (Arc<InMemConsensusStore<BlockHeader>>, Vec<BlockHeader>) {
        let headers: Vec<BlockHeader> = run(any_headers_chain(n));
        let store = Arc::new(InMemConsensusStore::new());
        for h in &headers {
            store.store_header(h).unwrap();
            store.roll_forward_chain(&h.point()).unwrap();
            store.set_best_chain_hash(&h.hash()).unwrap();
        }
        (store, headers)
    }
}
