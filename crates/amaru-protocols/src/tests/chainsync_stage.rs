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

use std::{collections::VecDeque, sync::Arc, time::Duration};

use amaru_kernel::{BlockHeader, IsHeader, Point, cbor};
use amaru_ouroboros_traits::Nonces;
use amaru_pure_stage::{Effects, StageRef};
use tokio::sync::Notify;

use crate::{
    blockfetch::Blocks, chainsync, chainsync::ChainSyncInitiatorMsg, manager::ManagerMessage, store_effects::Store,
    tests::configuration::RESPONDER_BLOCKS_NB,
};

/// State for the ChainSync stage
/// The stage batches block fetch requests to test the manager's block fetch capabilities with the Message::RequestRange variant.
/// We accumulate the next points to fetch in this state and keep track of the total number of requested blocks.
#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct ChainSyncStageState {
    manager: StageRef<ManagerMessage>,
    fetcher: StageRef<StoreFetchedBlocksMessage>,
    blocks_to_fetch: Vec<Point>,
    total_requested_blocks: usize,
    processing_wait: Option<Duration>,
}

impl ChainSyncStageState {
    pub(super) fn new(
        manager: StageRef<ManagerMessage>,
        fetcher: StageRef<StoreFetchedBlocksMessage>,
        processing_wait: Option<Duration>,
    ) -> Self {
        Self { manager, fetcher, blocks_to_fetch: Vec::new(), total_requested_blocks: 0, processing_wait }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct FetchBatch {
    from: Point,
    through: Point,
    expected_points: VecDeque<Point>,
    handler: StageRef<chainsync::InitiatorMessage>,
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub(super) enum StoreFetchedBlocksMessage {
    FetchBatch(FetchBatch),
    Blocks(Blocks),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct PendingFetch {
    id: u64,
    remaining_points: VecDeque<Point>,
    handler: StageRef<chainsync::InitiatorMessage>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(super) struct StoreFetchedBlocks {
    manager: StageRef<ManagerMessage>,
    blocks: StageRef<Blocks>,
    next_id: u64,
    current: Option<PendingFetch>,
    queue: VecDeque<FetchBatch>,
    total_requested_blocks: usize,
    #[serde(skip)]
    notify: Arc<Notify>,
}

impl PartialEq for StoreFetchedBlocks {
    fn eq(&self, other: &Self) -> bool {
        self.manager == other.manager
            && self.blocks == other.blocks
            && self.next_id == other.next_id
            && self.current == other.current
            && self.queue == other.queue
            && self.total_requested_blocks == other.total_requested_blocks
    }
}

impl Eq for StoreFetchedBlocks {}

impl StoreFetchedBlocks {
    pub(super) fn new(manager: StageRef<ManagerMessage>, blocks: StageRef<Blocks>, notify: Arc<Notify>) -> Self {
        Self { manager, blocks, next_id: 0, current: None, queue: VecDeque::new(), total_requested_blocks: 0, notify }
    }
}

async fn start_next_fetch(state: &mut StoreFetchedBlocks, eff: &Effects<StoreFetchedBlocksMessage>) {
    if state.current.is_some() {
        return;
    }

    let Some(batch) = state.queue.pop_front() else {
        return;
    };

    state.next_id += 1;
    let id = state.next_id;
    state.total_requested_blocks += batch.expected_points.len();
    state.current = Some(PendingFetch { id, remaining_points: batch.expected_points, handler: batch.handler });
    eff.send(
        &state.manager,
        ManagerMessage::FetchBlocks { from: batch.from, through: batch.through, cr: state.blocks.clone(), id },
    )
    .await;
}

async fn advance_after_fetch(
    state: &mut StoreFetchedBlocks,
    handler: StageRef<chainsync::InitiatorMessage>,
    eff: &Effects<StoreFetchedBlocksMessage>,
) {
    if state.total_requested_blocks == RESPONDER_BLOCKS_NB - 1 {
        tracing::info!("all blocks retrieved, done");
        state.notify.notify_waiters();
    } else if state.queue.is_empty() {
        eff.send(&handler, chainsync::InitiatorMessage::RequestNext).await;
    } else {
        start_next_fetch(state, eff).await;
    }
}

pub(super) async fn store_fetched_blocks(
    mut state: StoreFetchedBlocks,
    msg: StoreFetchedBlocksMessage,
    eff: Effects<StoreFetchedBlocksMessage>,
) -> StoreFetchedBlocks {
    match msg {
        StoreFetchedBlocksMessage::FetchBatch(batch) => {
            state.queue.push_back(batch);
            start_next_fetch(&mut state, &eff).await;
        }
        StoreFetchedBlocksMessage::Blocks(Blocks::NoBlocks(id)) => {
            if !matches!(state.current.as_ref(), Some(current) if current.id == id) {
                return state;
            }
            let current = state.current.take().expect("current fetch must exist");
            assert!(current.remaining_points.is_empty(), "expected blocks for request {id}, got no blocks");
            advance_after_fetch(&mut state, current.handler, &eff).await;
        }
        StoreFetchedBlocksMessage::Blocks(Blocks::NoPeersAvailable(id)) => {
            panic!("unexpected NoPeersAvailable for request {id}: test expects connected peers");
        }
        StoreFetchedBlocksMessage::Blocks(Blocks::Block(id, _peer, network_block)) => {
            if !matches!(state.current.as_ref(), Some(current) if current.id == id) {
                return state;
            }
            let block_header = network_block.decode_header().expect("failed to extract header from block");
            let current = state.current.as_mut().expect("current fetch must exist");
            let expected_point = current.remaining_points.pop_front().expect("unexpected extra block");
            assert_eq!(block_header.point(), expected_point, "unexpected block point for request {id}");
            tracing::info!("storing block {:?}", block_header.point());
            Store::new(eff.clone()).store_block(&block_header.hash(), &network_block.raw_block()).await.unwrap();
        }
        StoreFetchedBlocksMessage::Blocks(Blocks::Done(id)) => {
            if !matches!(state.current.as_ref(), Some(current) if current.id == id) {
                return state;
            }
            let current = state.current.take().expect("current fetch must exist");
            assert!(current.remaining_points.is_empty(), "missing blocks for request {id}");
            advance_after_fetch(&mut state, current.handler, &eff).await;
        }
    }

    state
}

/// This is a simplified version of the chain sync processing
/// that only stores headers and fetches blocks in batches of 3.
/// There is no validation or chain selection logic here.
pub(super) async fn test_chainsync_stage(
    mut state: ChainSyncStageState,
    msg: ChainSyncInitiatorMsg,
    eff: Effects<ChainSyncInitiatorMsg>,
) -> ChainSyncStageState {
    use crate::chainsync::InitiatorResult::*;
    match msg.msg {
        Initialize => {
            tracing::info!(peer = %msg.peer,"initializing chainsync");
        }
        IntersectFound(point, tip) => {
            tracing::info!(peer = %msg.peer, %point, %tip, "intersect found");
        }
        IntersectNotFound(tip) => {
            tracing::info!(peer = %msg.peer, %tip, "intersect not found");
            eff.send(&msg.handler, chainsync::InitiatorMessage::Done).await;
        }
        RollForward(header_content, tip) => {
            let block_header: BlockHeader = cbor::from_cbor(header_content.cbor.as_slice()).unwrap();
            let header_hash = block_header.hash();
            let point = block_header.point();
            let store = Store::new(eff.clone());
            let peer = msg.peer;
            tracing::info!(%peer, hash = header_hash.to_string(), %tip, "roll forward");

            // store the header, update the best chain, fetch and store the block
            store.store_validated_header(&block_header, &Nonces::for_tests()).await.unwrap();
            store.roll_forward_chain(&point).await.unwrap();
            // We accumulate points to fetch and fetch them in batches of 3
            state.blocks_to_fetch.push(point);

            // By construction the initiator and the responder just have 1 block in common
            // so we know that we eventually need to fetch RESPONDER_BLOCKS_NB - 1 blocks.
            let remaining_number_of_blocks_to_retrieve = RESPONDER_BLOCKS_NB - 1 - state.total_requested_blocks;

            // If the last batch isn't full but would allow us to complete the retrieval, we fetch it as well.
            if state.blocks_to_fetch.len() == 3 || state.blocks_to_fetch.len() == remaining_number_of_blocks_to_retrieve
            {
                let from = *state.blocks_to_fetch.first().unwrap();
                let through = *state.blocks_to_fetch.last().unwrap();
                let expected_blocks = state.blocks_to_fetch.len();
                state.total_requested_blocks += expected_blocks;
                eff.send(
                    &state.fetcher,
                    StoreFetchedBlocksMessage::FetchBatch(FetchBatch {
                        from,
                        through,
                        expected_points: state.blocks_to_fetch.iter().copied().collect(),
                        handler: msg.handler.clone(),
                    }),
                )
                .await;
                state.blocks_to_fetch.clear();
            } else {
                eff.send(&msg.handler, chainsync::InitiatorMessage::RequestNext).await;
            }
            if let Some(wait_time) = state.processing_wait {
                eff.wait(wait_time).await;
            }
            return state;
        }
        RollBackward(point, tip) => {
            tracing::info!(peer = %msg.peer, %point, %tip, "roll backward");
            eff.send(&msg.handler, chainsync::InitiatorMessage::RequestNext).await;
        }
        Terminated => {
            tracing::info!(peer = %msg.peer, "chainsync terminated");
        }
    }
    state
}
