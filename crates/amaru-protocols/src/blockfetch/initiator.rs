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
    blockfetch::{State, messages::Message},
    mux::MuxMessage,
    protocol::{
        Initiator, Inputs, Miniprotocol, Outcome, PROTO_N2N_BLOCK_FETCH, ProtocolState, StageState,
        miniprotocol, outcome,
    },
};
use amaru_kernel::{BlockHeader, IsHeader, Point, peer::Peer};
use amaru_ouroboros::ConnectionId;
use pure_stage::{DeserializerGuards, Effects, StageRef, Void};
use std::{collections::VecDeque, mem};

pub fn register_deserializers() -> DeserializerGuards {
    vec![
        pure_stage::register_data_deserializer::<BlockFetchInitiator>().boxed(),
        pure_stage::register_data_deserializer::<BlockFetchMessage>().boxed(),
        pure_stage::register_data_deserializer::<Blocks>().boxed(),
    ]
}

pub fn initiator() -> Miniprotocol<State, BlockFetchInitiator, Initiator> {
    miniprotocol(PROTO_N2N_BLOCK_FETCH)
}

#[derive(Default, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
pub struct Blocks {
    pub blocks: Vec<Vec<u8>>,
}

impl std::fmt::Debug for Blocks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Blocks")
            .field("blocks", &self.blocks.len())
            .field("first_block", &self.blocks.first().map(|b| b.len()))
            .finish()
    }
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BlockFetchMessage {
    RequestRange {
        from: Point,
        through: Point,
        cr: StageRef<Blocks>,
    },
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BlockFetchInitiator {
    muxer: StageRef<MuxMessage>,
    peer: Peer,
    conn_id: ConnectionId,
    /// Queue of requests that have been received but not yet answered.
    ///
    /// Note that the first two elements of the queue have already been sent
    /// to the network (pipelining).
    queue: VecDeque<(Point, Point, StageRef<Blocks>)>,
    blocks: Vec<Vec<u8>>,
}

impl BlockFetchInitiator {
    pub fn new(muxer: StageRef<MuxMessage>, peer: Peer, conn_id: ConnectionId) -> (State, Self) {
        (
            State::Idle,
            Self {
                muxer,
                peer,
                conn_id,
                queue: VecDeque::new(),
                blocks: Vec::new(),
            },
        )
    }

    /// Return true if the provided blocks form a valid chain from `from` to `through`.
    /// This includes checks that:
    /// - The first block matches the `from` point.
    /// - The last block matches the `through` point.
    /// - Each block's slot is strictly greater than the previous block's slot.
    /// - Each block's parent hash matches the hash of the previous block.
    #[expect(clippy::expect_used)]
    pub fn is_valid_block_range(blocks: &[Vec<u8>], from: Point, through: Point) -> bool {
        assert!(
            !blocks.is_empty(),
            "some blocks should have been fetched from {from} to {through}"
        );

        // Extract headers from all blocks
        let mut headers = Vec::with_capacity(blocks.len());
        for (idx, block_bytes) in blocks.iter().enumerate() {
            match BlockHeader::extract_header(block_bytes) {
                Ok(header) => {
                    headers.push(header);
                }
                Err(e) => {
                    tracing::warn!(
                        block_index = idx,
                        error = %e,
                        "failed to extract header from block in range validation"
                    );
                    return false;
                }
            }
        }

        // Validate first block matches 'from' point
        let first_point = headers.first().expect("non-empty headers").point();
        if first_point != from {
            tracing::debug!(
                ?from,
                actual = ?first_point,
                "first block does not match 'from' point"
            );
            return false;
        }

        // Validate last block matches 'through' point
        let last_point = headers.last().expect("non-empty headers").point();
        if last_point != through {
            tracing::debug!(
                ?through,
                actual = ?last_point,
                "last block does not match 'through' point"
            );
            return false;
        }

        // Validate chain continuity: slots increase and parent hashes match
        for window in headers.windows(2) {
            let parent = &window[0];
            let child = &window[1];

            // Check slots are strictly increasing (gaps are OK)
            if child.slot() <= parent.slot() {
                tracing::debug!(
                    parent_point = ?parent.point(),
                    child_point = ?child.point(),
                    "blocks are not in ascending slot order"
                );
                return false;
            }

            // Check parent-child hash relationship
            let expected_parent_hash = Some(parent.hash());
            let actual_parent_hash = child.parent_hash();
            if actual_parent_hash != expected_parent_hash {
                tracing::debug!(
                    parent_hash = ?parent.hash(),
                    child_parent_hash = ?actual_parent_hash,
                    child_point = ?child.point(),
                    "child block's parent hash does not match previous block's hash"
                );
                return false;
            }
        }

        true
    }
}

impl StageState<State, Initiator> for BlockFetchInitiator {
    type LocalIn = BlockFetchMessage;

    async fn local(
        mut self,
        _proto: &State,
        input: Self::LocalIn,
        _eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<InitiatorAction>, Self)> {
        match input {
            BlockFetchMessage::RequestRange { from, through, cr } => {
                let action = (self.queue.len() < 2)
                    .then_some(InitiatorAction::RequestRange { from, through });
                self.queue.push_back((from, through, cr));
                Ok((action, self))
            }
        }
    }

    #[expect(clippy::expect_used)]
    async fn network(
        mut self,
        _proto: &State,
        input: InitiatorResult,
        eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<InitiatorAction>, Self)> {
        let queued = match input {
            InitiatorResult::Initialize => None,
            InitiatorResult::NoBlocks => {
                let (_, _, cr) = self.queue.pop_front().expect("queue is empty");
                eff.send(&cr, Blocks { blocks: Vec::new() }).await;
                self.queue.get(1)
            }
            InitiatorResult::Block(body) => {
                self.blocks.push(body);
                None
            }
            InitiatorResult::Done => {
                let (from, through, cr) = self.queue.pop_front().expect("queue is empty");
                let blocks = mem::take(&mut self.blocks);
                if BlockFetchInitiator::is_valid_block_range(&blocks, from, through) {
                    eff.send(&cr, Blocks { blocks }).await;
                } else {
                    tracing::warn!(
                        ?from,
                        ?through,
                        "received blocks do not form a valid range; terminating the connection"
                    );
                    eff.terminate::<Void>().await;
                }
                self.queue.get(1)
            }
        };
        let action = queued.map(|(from, through, _)| InitiatorAction::RequestRange {
            from: *from,
            through: *through,
        });
        Ok((action, self))
    }

    fn muxer(&self) -> &StageRef<MuxMessage> {
        &self.muxer
    }
}

impl ProtocolState<Initiator> for State {
    type WireMsg = Message;
    type Action = InitiatorAction;
    type Out = InitiatorResult;
    type Error = Void;

    fn init(&self) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out, Self::Error>, Self)> {
        Ok((outcome().result(InitiatorResult::Initialize), *self))
    }

    fn network(
        &self,
        input: Self::WireMsg,
    ) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out, Self::Error>, Self)> {
        use Message::*;
        match (self, input) {
            (Self::Busy, StartBatch) => Ok((outcome().want_next(), Self::Streaming)),
            (Self::Busy, NoBlocks) => Ok((outcome().result(InitiatorResult::NoBlocks), Self::Idle)),
            (Self::Streaming, Block { body }) => Ok((
                outcome().want_next().result(InitiatorResult::Block(body)),
                Self::Streaming,
            )),
            (Self::Streaming, BatchDone) => {
                Ok((outcome().result(InitiatorResult::Done), Self::Idle))
            }
            (state, msg) => anyhow::bail!("unexpected message in state {:?}: {:?}", state, msg),
        }
    }

    fn local(
        &self,
        input: Self::Action,
    ) -> anyhow::Result<(Outcome<Self::WireMsg, Void, Self::Error>, Self)> {
        use InitiatorAction::*;
        match (self, input) {
            (Self::Idle, RequestRange { from, through }) => Ok((
                outcome()
                    .send(Message::RequestRange { from, through })
                    .want_next(),
                Self::Busy,
            )),
            (Self::Idle, ClientDone) => Ok((outcome().send(Message::ClientDone), Self::Done)),
            (state, action) => {
                anyhow::bail!("unexpected action in state {:?}: {:?}", state, action)
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
pub enum InitiatorResult {
    Initialize,
    NoBlocks,
    Block(Vec<u8>),
    Done,
}

#[derive(Debug, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
pub enum InitiatorAction {
    RequestRange { from: Point, through: Point },
    ClientDone,
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::protocol::Initiator;
    use amaru_kernel::is_header::tests::{any_headers_chain, make_header, run};
    use amaru_kernel::{BlockHeader, HeaderHash, cbor};

    #[test]
    #[expect(clippy::wildcard_enum_match_arm)]
    fn test_initiator_protocol() {
        crate::blockfetch::spec::<Initiator>().check(State::Idle, |msg| match msg {
            Message::RequestRange { from, through } => Some(InitiatorAction::RequestRange {
                from: *from,
                through: *through,
            }),
            Message::ClientDone => Some(InitiatorAction::ClientDone),
            _ => None,
        });
    }

    #[test]
    fn test_valid_block_range_single_block() {
        let headers = run(any_headers_chain(1));
        let blocks = vec![make_block(&headers[0])];

        assert!(BlockFetchInitiator::is_valid_block_range(
            &blocks,
            headers[0].point(),
            headers[0].point()
        ));
    }

    #[test]
    fn test_valid_block_range_consecutive_blocks() {
        let headers = run(any_headers_chain(3));
        let blocks = vec![
            make_block(&headers[0]),
            make_block(&headers[1]),
            make_block(&headers[2]),
        ];

        assert!(BlockFetchInitiator::is_valid_block_range(
            &blocks,
            headers[0].point(),
            headers[2].point()
        ));
    }

    #[test]
    #[should_panic(expected = "some blocks should have been fetched")]
    fn test_empty_blocks_with_equal_range() {
        let headers = run(any_headers_chain(1));
        BlockFetchInitiator::is_valid_block_range(&[], headers[0].point(), headers[0].point());
    }

    #[test]
    fn test_first_block_point_mismatch() {
        // Create blocks where the first block doesn't match 'from'
        let header1 = make_header(1, 100, None);
        let block_header1 = BlockHeader::from(header1.clone());

        let header2 = make_header(2, 101, Some(block_header1.hash()));
        let block_header2 = BlockHeader::from(header2.clone());
        let point2 = block_header2.point();

        let blocks = vec![make_block(&block_header1), make_block(&block_header2)];

        // Use a different 'from' point that doesn't match the first block
        let wrong_from = Point::Specific(99u64.into(), HeaderHash::from([99u8; 32]));
        assert!(!BlockFetchInitiator::is_valid_block_range(
            &blocks, wrong_from, point2
        ));
    }

    #[test]
    fn test_last_block_point_mismatch() {
        // Create blocks where the last block doesn't match 'through'
        let header1 = make_header(1, 100, None);
        let block_header1 = BlockHeader::from(header1.clone());
        let point1 = block_header1.point();

        let header2 = make_header(2, 101, Some(block_header1.hash()));
        let block_header2 = BlockHeader::from(header2.clone());

        let blocks = vec![make_block(&block_header1), make_block(&block_header2)];

        // Use a different 'through' point that doesn't match the last block
        let wrong_through = Point::Specific(102u64.into(), HeaderHash::from([102u8; 32]));
        assert!(!BlockFetchInitiator::is_valid_block_range(
            &blocks,
            point1,
            wrong_through
        ));
    }

    #[test]
    fn test_blocks_with_non_increasing_slots() {
        // Create blocks where slots are not strictly increasing
        let header1 = make_header(1, 100, None);
        let block_header1 = BlockHeader::from(header1.clone());
        let point1 = block_header1.point();

        let header2 = make_header(2, 99, Some(block_header1.hash())); // Slot goes backward!
        let block_header2 = BlockHeader::from(header2.clone());
        let point2 = block_header2.point();

        let blocks = vec![make_block(&block_header1), make_block(&block_header2)];

        assert!(!BlockFetchInitiator::is_valid_block_range(
            &blocks, point1, point2
        ));
    }

    #[test]
    fn test_blocks_with_equal_slots() {
        // Create blocks where slots are equal (should fail)
        let header1 = make_header(1, 100, None);
        let block_header1 = BlockHeader::from(header1.clone());
        let point1 = block_header1.point();

        let header2 = make_header(2, 100, Some(block_header1.hash())); // Same slot!
        let block_header2 = BlockHeader::from(header2.clone());
        let point2 = block_header2.point();

        let blocks = vec![make_block(&block_header1), make_block(&block_header2)];

        assert!(!BlockFetchInitiator::is_valid_block_range(
            &blocks, point1, point2
        ));
    }

    #[test]
    fn test_broken_parent_child_hash_chain() {
        // Create blocks where the parent hash doesn't match
        let header1 = make_header(1, 100, None);
        let block_header1 = BlockHeader::from(header1.clone());
        let point1 = block_header1.point();

        // Create header2 with wrong parent hash (not matching block_header1's hash)
        let wrong_parent_hash = HeaderHash::from([99u8; 32]);
        let header2 = make_header(2, 101, Some(wrong_parent_hash));
        let block_header2 = BlockHeader::from(header2.clone());
        // Use the actual point from block_header2 so we test parent-child hash validation
        let point2 = block_header2.point();

        let blocks = vec![make_block(&block_header1), make_block(&block_header2)];

        assert!(!BlockFetchInitiator::is_valid_block_range(
            &blocks, point1, point2
        ));
    }

    #[test]
    fn test_invalid_cbor_in_block() {
        // Create a valid first block and an invalid second block
        let header1 = make_header(1, 100, None);
        let block_header1 = BlockHeader::from(header1.clone());
        let point1 = block_header1.point();

        let blocks = vec![
            make_block(&block_header1),
            vec![0xFF, 0xFF, 0xFF], // Invalid CBOR
        ];

        let point2 = Point::Specific(101u64.into(), HeaderHash::from([2u8; 32]));
        assert!(!BlockFetchInitiator::is_valid_block_range(
            &blocks, point1, point2
        ));
    }

    // HELPERS

    fn make_block(header: &BlockHeader) -> Vec<u8> {
        let mut block_bytes = Vec::new();
        let mut encoder = cbor::Encoder::new(&mut block_bytes);

        // Network format: [era_tag, [header, tx_bodies, witnesses, auxiliary_data?, invalid_transactions?]]
        encoder.array(2).expect("failed to encode outer array");

        // Encode era tag
        encoder
            .u16(amaru_kernel::CONWAY_ERA_TAG)
            .expect("failed to encode era tag");

        // Encode MintedBlock
        encoder.array(5).expect("failed to encode inner array");
        encoder
            .encode(header.header())
            .expect("failed to encode header");
        encoder.array(0).expect("failed to encode tx bodies");
        encoder.array(0).expect("failed to encode witnesses");
        encoder.null().expect("failed to encode auxiliary data");
        encoder.null().expect("failed to encode invalid txs");

        block_bytes
    }
}
