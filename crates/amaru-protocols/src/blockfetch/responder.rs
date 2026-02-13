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
use amaru_kernel::{BlockHeader, IsHeader, NonEmptyVec, Point, RawBlock};
use amaru_ouroboros_traits::ChainStore;
use pure_stage::{DeserializerGuards, Effects, StageRef, Void};
use std::fmt::Debug;
use tracing::instrument;

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
/// The points are ordered from the most recent to oldest and at least one point is present
#[derive(Debug, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
pub struct PointsRange(NonEmptyVec<Point>);

/// Maximum number of blocks that can be streamed for a single request
pub const MAX_FETCHED_BLOCKS: usize = 1000;

impl PointsRange {
    /// Create a points range with a single point
    pub fn singleton(first: Point) -> PointsRange {
        PointsRange(NonEmptyVec::singleton(first))
    }

    /// Create a points range from a vector of points.
    pub fn from_vec(vec: Vec<Point>) -> Option<PointsRange> {
        NonEmptyVec::try_from(vec).ok().map(PointsRange)
    }

    #[cfg(test)]
    pub fn points(&self) -> Vec<Point> {
        self.0.to_vec()
    }

    /// Load the first available block in the current range (the block is expected to be found).
    /// Each time we attempt to fetch a block we pop its point from the current_range.
    fn next_block(
        self,
        store: &dyn ChainStore<BlockHeader>,
    ) -> anyhow::Result<(RawBlock, Option<PointsRange>)> {
        // points are stored from most recent to oldest, so we pop from the end
        let (last, rest) = self.0.pop();
        let last_hash = last.hash();
        let stored_block = store
            .load_block(&last_hash)?
            .ok_or_else(|| anyhow::anyhow!("block {} was pruned", last_hash))?;
        Ok((stored_block, rest.map(PointsRange)))
    }

    /// Return a points range:
    ///  - Check that `from` <= `through`
    ///  - Check that there is a valid path of block from `from` to `through` in the chain store.
    ///  - Check that we don't return too many headers to avoid getting over the protocol limits.
    ///  - Return None if any of the above checks fail and return the points range otherwise.
    pub fn request_range(
        store: &dyn ChainStore<BlockHeader>,
        from: Point,
        through: Point,
    ) -> anyhow::Result<Option<PointsRange>> {
        // make sure that from <= through
        if from > through {
            tracing::debug!(%from, %through, "requested range is invalid: from > through");
            return Ok(None);
        };

        if from == through {
            return if store.load_block(&from.hash())?.is_some() {
                Ok(Some(PointsRange::singleton(from)))
            } else {
                Ok(None)
            };
        }

        let mut current_hash = through.hash();
        let mut result = vec![];
        loop {
            if result.len() >= MAX_FETCHED_BLOCKS {
                tracing::debug!(
                    %from,
                    %through,
                    max_blocks = MAX_FETCHED_BLOCKS,
                    "requested range exceeds maximum allowed blocks"
                );
                return Ok(None);
            }
            // check that the block exists
            if store.load_block(&current_hash)?.is_none() {
                return Ok(None);
            }

            // load the header for the current hash
            if let Some(header) = store.load_header(&current_hash) {
                result.push(header.point());
                // if we found the from point, we're done
                if current_hash == from.hash() {
                    break;
                }
                // if we reached a slot before 'from', abort
                if header.slot() < from.slot_or_default() {
                    return Ok(None);
                }
                if let Some(parent_hash) = header.parent_hash() {
                    current_hash = parent_hash
                } else {
                    return Ok(None);
                }
            } else {
                return Ok(None);
            }
        }
        Ok(PointsRange::from_vec(result))
    }
}

impl BlockFetchResponder {
    pub fn new(muxer: StageRef<MuxMessage>) -> (State, Self) {
        (State::Idle, Self { muxer })
    }
}

/// Local message for streaming blocks.
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum StreamBlocks {
    More(PointsRange),
    Done,
}

impl StageState<State, Responder> for BlockFetchResponder {
    type LocalIn = StreamBlocks;

    async fn local(
        self,
        _proto: &State,
        input: Self::LocalIn,
        eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<ResponderAction>, Self)> {
        let store = Store::new(eff.clone());
        match input {
            StreamBlocks::Done => Ok((Some(ResponderAction::BatchDone), self)),
            StreamBlocks::More(points_range) => {
                let (block, points_range) = points_range.next_block(&store)?;
                // recurse if there are more blocks to fetch or signal that streaming is done
                if let Some(points_range) = points_range {
                    eff.send(
                        eff.me_ref(),
                        Inputs::Local(StreamBlocks::More(points_range)),
                    )
                    .await;
                } else {
                    eff.send(eff.me_ref(), Inputs::Local(StreamBlocks::Done))
                        .await;
                }
                Ok((Some(ResponderAction::Block(block)), self))
            }
        }
    }

    #[instrument(name = "blockfetch.responder.stage", skip_all, fields(message_type = input.message_type()))]
    async fn network(
        self,
        _proto: &State,
        input: ResponderResult,
        eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<ResponderAction>, Self)> {
        match input {
            ResponderResult::RequestRange { from, through } => {
                let store = Store::new(eff.clone());
                if let Some(points_range) = PointsRange::request_range(&store, from, through)? {
                    eff.send(
                        eff.me_ref(),
                        Inputs::Local(StreamBlocks::More(points_range)),
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

    #[instrument(name = "blockfetch.responder.protocol", skip_all, fields(message_type = input.message_type()))]
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
            (Self::Streaming, Block(body)) => Ok((
                outcome().send(Message::Block {
                    body: body.to_vec(),
                }),
                Self::Streaming,
            )),
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
    Block(RawBlock),
    BatchDone,
}

#[derive(Debug, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
pub enum ResponderResult {
    RequestRange { from: Point, through: Point },
    Done,
}

impl ResponderResult {
    pub fn message_type(&self) -> &'static str {
        match self {
            ResponderResult::RequestRange { .. } => "RequestRange",
            ResponderResult::Done => "Done",
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::protocol::Responder;
    use amaru_kernel::cardano::network_block::{NetworkBlock, make_encoded_block};
    use amaru_kernel::{
        BlockHeader, IsHeader, TESTNET_ERA_HISTORY, any_fake_header, any_headers_chain,
        any_headers_chain_with_root, utils::tests::run_strategy,
    };
    use amaru_kernel::{EraName, Slot};
    use amaru_ouroboros_traits::ChainStore;
    use amaru_ouroboros_traits::in_memory_consensus_store::InMemConsensusStore;
    use std::sync::Arc;

    #[test]
    #[expect(clippy::wildcard_enum_match_arm)]
    fn test_responder_protocol() {
        crate::blockfetch::spec::<Responder>().check(State::Idle, |msg| match msg {
            Message::NoBlocks => Some(ResponderAction::NoBlocks),
            Message::StartBatch => Some(ResponderAction::StartBatch),
            Message::Block { body } => {
                Some(ResponderAction::Block(RawBlock::from(body.as_slice())))
            }
            Message::BatchDone => Some(ResponderAction::BatchDone),
            _ => None,
        });
    }

    #[test]
    fn decode_network_block() {
        let as_hex = "820785828a1a002cc8f51a04994d195820f27eddec5e782552e6ef408cff7c4a27e505fe54c20a717027d97e1c91da9d7c5820064effe4fa426184a911159fa803a9c1092459cd0b8f3e584ef9513955be0f5558201e5d0dcf77643d89a94353493859a21b47672015fb652b51f922617e4b27da8982584042d0edd71e6cac29e45f61eabbcce4f803f2ff78bce9fa295d11cb7c3cddb60f7694faaea787183fd604267d8114b57453493c963c7485405838cd79a261013a5850bc8672b4ff2db478e5b21364bfa9f0a2f5265e5ac56b261ce3dcb7ac57301a8362573eef2ae23eb2540915704534d1c0af8eace59a25c130629af7600b175b5e234b376961e2fd12b37de5213e8eff0304582029571d16f081709b3c48651860077bebf9340abb3fc7133443c54f1f5a5edcf1845820ee1d7c2bd6978e3bc8a47fc478424a9efd797f16813164db292320e3728f6de5091902465840f69f8974108be5df23dd0dad2f0e888e5c1702c35c678f3b7a2802f272666ea8a7c9b9f6e786e761d4cb747159d68b7d8f43bceae6ab4e543795d8aded59c302820a005901c06063a37f6f01765b34bceb2651e40a69e3bc31b35fd6c952415175844132250cdcbafd19c39952f471f7318a5cc3e45f54dadc9067bb6d25dac8b76f0bea5106c2f45235fac710d3e78d259af37fd617ed9e372626c5b080359ba1bf5150df764365e0faedfe66ab7e338f7aec558e0a192f4f744b473fbe669013ade2cd144c7742c3ff1d78002af59b0f1b45807bce21f592d23596c54d37095b52a8f942c763f5f014aa161fc18123054a618e8ecb9256c392c3bebcb30e10b2c4bef64f4c3b0aea29a4378a53b6d061c9000b510c0bf76d87171fb357faeb54087718fea0ee33e048d4a1aa8a831f7f9148ebbbb2d79f58c61268e1e1369ae88e2369e65e57169cc477726944790423f9dee584fb9eceeee79a447c075ada7bceb6a28699f0721415d3d0ab8f20b77410bc5faf296ce126cb73b9aaab208b9844d95d127ccaefac37c323cc1957aad3350c2d176916593aa854be50e7c36857adcf51800d490ce082908c5a1aceb8fd51fffc67abaf2c09c1f957bc2e009b8a76394402211eac5ff26c2e5d69aa2c6f4a0e4f2ac28c1482b4706916a0c876d56952b1db18af64658f6249db7fe7e7e366fd2a0f869472d38edb6145404f556025ea0066228080a080";
        let bytes = hex::decode(as_hex).expect("valid hex");
        let network_block: NetworkBlock = minicbor::decode(&bytes).expect("a valid network block");
        assert_eq!(network_block.era_tag(), EraName::Conway);
    }

    #[test]
    fn test_request_range_invalid_from_greater_than_through() {
        let (store, headers) = make_store_with_chain(5);
        let result =
            PointsRange::request_range(&*store, headers[3].point(), headers[1].point()).unwrap();
        assert_eq!(result, None, "should return None when from > through");
    }

    #[test]
    fn test_request_range_single_point_block_exists() {
        let (store, headers) = make_store_with_chain(3);
        store_blocks(store.clone(), &headers[1..2]);

        let result =
            PointsRange::request_range(&*store, headers[1].point(), headers[1].point()).unwrap();
        assert_eq!(result, Some(PointsRange::singleton(headers[1].point())));
    }

    #[test]
    fn test_request_range_single_point_block_missing() {
        let (store, headers) = make_store_with_chain(3);
        let result =
            PointsRange::request_range(&*store, headers[1].point(), headers[1].point()).unwrap();
        assert_eq!(
            result, None,
            "should return None when from == through but block doesn't exist"
        );
    }

    #[test]
    fn test_request_range_valid_chain() {
        let (store, headers) = make_store_with_chain(5);
        store_blocks(store.clone(), &headers);
        let result =
            PointsRange::request_range(&*store, headers[0].point(), headers[4].point()).unwrap();
        assert_eq!(
            result,
            PointsRange::from_vec(vec![
                headers[4].point(),
                headers[3].point(),
                headers[2].point(),
                headers[1].point(),
                headers[0].point(),
            ])
        );
    }

    #[test]
    fn test_request_range_missing_block_in_chain() {
        let (store, headers) = make_store_with_chain(5);

        // Store blocks for all headers except one in the middle
        for (i, h) in headers.iter().enumerate() {
            if i != 2 {
                // Skip storing block for index 2
                let raw_block = RawBlock::from(&[1u8, 2, 3][..]);
                store.store_block(&h.hash(), &raw_block).unwrap();
            }
        }

        let result =
            PointsRange::request_range(&*store, headers[0].point(), headers[4].point()).unwrap();
        assert_eq!(
            result, None,
            "should return None when a block is missing in the chain"
        );
    }

    #[test]
    fn test_request_range_missing_header_in_chain() {
        let headers: Vec<BlockHeader> = run_strategy(any_headers_chain(5));
        let store = Arc::new(InMemConsensusStore::new());

        // Set anchor to the first header
        store.set_anchor_hash(&headers[0].hash()).unwrap();

        // Store only some headers (skip one in the middle)
        for (i, h) in headers.iter().enumerate() {
            if i != 2 {
                // Skip storing header for index 2
                store.store_header(h).unwrap();
                store.roll_forward_chain(&h.point()).unwrap();
                store.set_best_chain_hash(&h.hash()).unwrap();
                let raw_block = RawBlock::from(&[1u8, 2, 3][..]);
                store.store_block(&h.hash(), &raw_block).unwrap();
            }
        }

        let result =
            PointsRange::request_range(&*store, headers[0].point(), headers[4].point()).unwrap();
        assert_eq!(
            result, None,
            "should return None when a header is missing in the chain"
        );
    }

    #[test]
    fn test_request_range_no_parent_hash_before_from() {
        let genesis = Point::Specific(Slot::from(10), run_strategy(any_fake_header()).hash());
        let (store, headers) = make_store_with_chain_starting_from(5, genesis);

        let result = PointsRange::request_range(
            &*store,
            Point::Specific(Slot::from(2), run_strategy(any_fake_header()).hash()),
            headers[3].point(),
        )
        .unwrap();
        assert_eq!(
            result, None,
            "should return None when we hit genesis before finding from"
        );
    }

    #[test]
    fn test_request_range_slot_before_from_abort() {
        // Create a chain with 5 headers
        let (store, headers) = make_store_with_chain(5);
        store_blocks(store.clone(), &headers);

        // Create a 'from' point that has a slot within the chain range but with a non-existent hash.
        // When traversing backwards from 'through', we'll pass the slot of 'from' without finding it,
        // and then hit a block with a slot before 'from', triggering the abort condition.
        let from_slot = headers[2].slot();
        let non_existent_hash = run_strategy(any_fake_header()).hash();
        let from = Point::Specific(from_slot, non_existent_hash);

        let result = PointsRange::request_range(&*store, from, headers[4].point()).unwrap();
        assert_eq!(
            result, None,
            "should return None when we reach a slot before 'from' without finding 'from'"
        );
    }

    #[test]
    fn test_request_range_exactly_max_blocks() {
        // Create a chain longer than MAX_BLOCKS
        let (store, headers) = make_store_with_chain(MAX_FETCHED_BLOCKS);
        store_blocks(store.clone(), &headers);

        let result = PointsRange::request_range(
            &*store,
            headers[0].point(),
            headers[MAX_FETCHED_BLOCKS - 1].point(),
        )
        .unwrap();

        assert_eq!(result.unwrap().points().len(), MAX_FETCHED_BLOCKS);
    }

    #[test]
    fn test_request_range_max_blocks_limit() {
        // Create a chain longer than MAX_BLOCKS
        let chain_length = MAX_FETCHED_BLOCKS + 1;
        let (store, headers) = make_store_with_chain(chain_length);
        store_blocks(store.clone(), &headers);

        let result = PointsRange::request_range(
            &*store,
            headers[0].point(),
            headers[chain_length - 1].point(),
        )
        .unwrap();
        assert_eq!(
            result, None,
            "should return None when the requested range exceeds MAX_BLOCKS limit"
        );
    }

    #[test]
    fn test_next_block_single_point() {
        let (store, headers) = make_store_with_chain(3);
        store_blocks(store.clone(), &headers);

        let (block, remaining_range) = PointsRange::singleton(headers[1].point())
            .next_block(&*store)
            .unwrap();

        // Should return the block for the single point
        let network_block: NetworkBlock = block.try_into().unwrap();
        assert_eq!(
            network_block.decode_header().unwrap().point(),
            headers[1].point()
        );

        // Should have no remaining range
        assert_eq!(remaining_range, None);
    }

    #[test]
    fn test_next_block_multiple_points() {
        let (store, headers) = make_store_with_chain(5);
        store_blocks(store.clone(), &headers);

        let (block, remaining_range) = PointsRange::from_vec(vec![
            headers[2].point(),
            headers[1].point(),
            headers[0].point(),
        ])
        .unwrap()
        .next_block(&*store)
        .unwrap();

        // Should return the first block
        let network_block: NetworkBlock = block.try_into().unwrap();
        assert_eq!(
            network_block.decode_header().unwrap().point(),
            headers[0].point()
        );

        // Should have remaining points
        assert_eq!(
            remaining_range,
            PointsRange::from_vec(vec![headers[2].point(), headers[1].point()])
        );
    }

    // HELPERS

    fn make_store_with_chain(
        n: usize,
    ) -> (Arc<InMemConsensusStore<BlockHeader>>, Vec<BlockHeader>) {
        make_store_with_chain_starting_from(n, Point::Origin)
    }

    fn make_store_with_chain_starting_from(
        n: usize,
        point: Point,
    ) -> (Arc<InMemConsensusStore<BlockHeader>>, Vec<BlockHeader>) {
        let headers: Vec<BlockHeader> = run_strategy(any_headers_chain_with_root(n, point));
        let store = Arc::new(InMemConsensusStore::new());
        // Set anchor to the first header
        store.set_anchor_hash(&headers[0].hash()).unwrap();
        for h in &headers {
            store.store_header(h).unwrap();
            store.roll_forward_chain(&h.point()).unwrap();
            store.set_best_chain_hash(&h.hash()).unwrap();
        }
        (store, headers)
    }

    fn store_blocks(store: Arc<InMemConsensusStore<BlockHeader>>, headers: &[BlockHeader]) {
        for h in headers {
            let raw_block = make_encoded_block(h, &TESTNET_ERA_HISTORY);
            store.store_block(&h.hash(), &raw_block).unwrap();
        }
    }
}
