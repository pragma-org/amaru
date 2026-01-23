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

use crate::chainsync;
use crate::chainsync::ChainSyncInitiatorMsg;
use crate::manager::ManagerMessage;
use crate::store_effects::Store;
use crate::tests::configuration::RESPONDER_BLOCKS_NB;
use amaru_kernel::{BlockHeader, Header, IsHeader, Point, cbor};
use amaru_ouroboros_traits::ChainStore;
use pallas_primitives::babbage::MintedHeader;
use pure_stage::{Effects, StageRef, TryInStage};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

/// State for the ChainSync stage
/// The stage batches block fetch requests to test the manager's block fetch capabilities with the Message::RequestRange variant.
/// We accumulate the next points to fetch in this state and keep track of the total number of requested blocks.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub(super) struct ChainSyncStageState {
    manager: StageRef<ManagerMessage>,
    blocks_to_fetch: Vec<Point>,
    total_requested_blocks: usize,
    processing_wait: Option<Duration>,
    #[serde(skip)]
    notify: Arc<Notify>,
}

impl PartialEq for ChainSyncStageState {
    fn eq(&self, other: &Self) -> bool {
        self.manager == other.manager
            && self.blocks_to_fetch == other.blocks_to_fetch
            && self.total_requested_blocks == other.total_requested_blocks
            && self.processing_wait == other.processing_wait
    }
}

impl Eq for ChainSyncStageState {}

impl ChainSyncStageState {
    pub(super) fn new(
        manager: StageRef<ManagerMessage>,
        processing_wait: Option<Duration>,
        notify: Arc<Notify>,
    ) -> Self {
        Self {
            manager,
            blocks_to_fetch: Vec::new(),
            total_requested_blocks: 0,
            processing_wait,
            notify,
        }
    }
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
            eff.send(&msg.handler, chainsync::InitiatorMessage::Done)
                .await;
        }
        RollForward(header_content, tip) => {
            let minted_header: MintedHeader<'_> =
                cbor::decode(header_content.cbor.as_slice()).unwrap();
            let header = Header::from(minted_header);
            let block_header = BlockHeader::from(header);
            let header_hash = block_header.hash();
            let point = block_header.point();
            let store = Store::new(eff.clone());
            let peer = msg.peer;
            tracing::info!(%peer, hash = header_hash.to_string(), %tip, "roll forward");

            // store the header, update the best chain, fetch and store the block
            store.store_header(&block_header).unwrap();
            store.roll_forward_chain(&point).unwrap();
            store.set_best_chain_hash(&header_hash).unwrap();

            // We accumulate points to fetch and fetch them in batches of 3
            state.blocks_to_fetch.push(point);

            // By construction the initiator and the responder just have 1 block in common
            // so we know that we eventually need to fetch RESPONDER_BLOCKS_NB - 1 blocks.
            let remaining_number_of_blocks_to_retrieve =
                RESPONDER_BLOCKS_NB - 1 - state.total_requested_blocks;

            // If the last batch isn't full but would allow us to complete the retrieval, we fetch it as well.
            if state.blocks_to_fetch.len() == 3
                || state.blocks_to_fetch.len() == remaining_number_of_blocks_to_retrieve
            {
                let from = *state.blocks_to_fetch.first().unwrap();
                let through = *state.blocks_to_fetch.last().unwrap();
                let blocks = eff
                    .call(&state.manager, Duration::from_secs(200), move |cr| {
                        ManagerMessage::FetchBlocks {
                            peer,
                            from,
                            through,
                            cr,
                        }
                    })
                    .await
                    .or_terminate(&eff, async |_| tracing::error!("failed to fetch blocks"))
                    .await;

                state.total_requested_blocks += state.blocks_to_fetch.len();
                // store the fetched blocks with their corresponding headers.
                tracing::info!("retrieved {} blocks", blocks.blocks.len());
                for network_block in blocks.blocks {
                    let block_header = network_block
                        .decode_header()
                        .expect("failed to extract header from block");
                    tracing::info!("storing block {:?}", block_header.point());
                    store
                        .store_block(&block_header.hash(), &network_block.raw_block())
                        .unwrap();
                }
                state.blocks_to_fetch.clear();
            };

            if state.total_requested_blocks == RESPONDER_BLOCKS_NB - 1 {
                tracing::info!("all blocks retrieved, done");
                state.notify.notify_waiters();
            } else {
                eff.send(&msg.handler, chainsync::InitiatorMessage::RequestNext)
                    .await;
            }
            if let Some(wait_time) = state.processing_wait {
                eff.wait(wait_time).await;
            }
            return state;
        }
        RollBackward(point, tip) => {
            tracing::info!(peer = %msg.peer, %point, %tip, "roll backward");
            eff.send(&msg.handler, chainsync::InitiatorMessage::RequestNext)
                .await;
        }
    }
    state
}
