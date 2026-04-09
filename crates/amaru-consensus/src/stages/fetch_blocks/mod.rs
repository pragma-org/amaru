// Copyright 2026 PRAGMA
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

use std::time::Duration;

use amaru_kernel::{BlockHeader, BlockHeight, IsHeader, Point, Tip, cardano::network_block::NetworkBlock};
use amaru_ouroboros_traits::MissingBlockRange;
use amaru_protocols::{blockfetch::Blocks2, manager::ManagerMessage, store_effects::Store};
use pure_stage::{Effects, ScheduleId, StageRef, TryInStage};

use crate::stages::select_chain::SelectChainMsg;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FetchBlocks {
    downstream: StageRef<(Tip, Point, BlockHeight)>,
    req_id: u64,
    missing_range: Option<MissingBlockRange>,
    upstream: StageRef<SelectChainMsg>,
    manager: StageRef<ManagerMessage>,
    cleanup_replies: StageRef<Blocks2>,
    timeout: Option<ScheduleId>,
    block_height: BlockHeight,
}

impl FetchBlocks {
    pub fn new(
        downstream: StageRef<(Tip, Point, BlockHeight)>,
        upstream: StageRef<SelectChainMsg>,
        manager: StageRef<ManagerMessage>,
    ) -> Self {
        Self {
            downstream,
            req_id: 0,
            missing_range: None,
            upstream,
            manager,
            cleanup_replies: StageRef::blackhole(),
            timeout: None,
            block_height: BlockHeight::from(0),
        }
    }

    /// Constructor for tests: use a mock cleanup_replies stage instead of wiring the real one.
    #[cfg(test)]
    pub fn for_tests(
        downstream: StageRef<(Tip, Point, BlockHeight)>,
        upstream: StageRef<SelectChainMsg>,
        manager: StageRef<ManagerMessage>,
        cleanup_replies: StageRef<Blocks2>,
    ) -> Self {
        Self {
            downstream,
            req_id: 0,
            missing_range: None,
            upstream,
            manager,
            cleanup_replies,
            timeout: None,
            block_height: BlockHeight::from(0),
        }
    }

    #[expect(clippy::expect_used)]
    pub async fn new_tip(&mut self, tip: Tip, parent: Point, eff: Effects<FetchBlocksMsg>) {
        self.block_height = tip.block_height().max(self.block_height);

        tracing::debug!(tip = %tip.point(), parent = %parent, "fetching blocks");
        let store = Store::new(eff.clone());
        assert!(
            self.missing_range.is_none(),
            "there shouldn't be any missing block range when starting a new tip: {:?}",
            self.missing_range
        );

        match store.find_missing_blocks(tip.hash(), 10).await {
            Ok(None) => {
                tracing::error!("failed to load initial header");
                return eff.terminate().await;
            }
            Ok(Some(range)) => {
                self.missing_range = Some(range);
            }
            Err(error) => {
                tracing::error!(%error, "failed to find missing blocks");
                return eff.terminate().await;
            }
        }
        let Some(range) = self.missing_range.as_ref() else {
            return;
        };
        if range.is_empty() {
            self.missing_range = None;
            tracing::info!(tip = %tip.point(), parent = %parent, "no blocks to fetch");
            eff.send(&self.upstream, SelectChainMsg::FetchNextFrom(tip.point())).await;
            return;
        }
        let from = range.first().expect("checked above that this exists");
        let through = range.last().expect("checked above that not empty");
        tracing::debug!(%from, %through, length = range.nb_missing_blocks(), "requesting blocks");
        self.req_id += 1;
        eff.send(
            &self.manager,
            ManagerMessage::FetchBlocks2 { from, through, id: self.req_id, cr: self.cleanup_replies.clone() },
        )
        .await;
        let timeout = eff.schedule_after(FetchBlocksMsg::Timeout(self.req_id), Duration::from_secs(5)).await;
        self.timeout = Some(timeout);
    }

    pub async fn block(&mut self, network_block: NetworkBlock, eff: Effects<FetchBlocksMsg>) {
        let store = Store::new(eff.clone());
        let block = match network_block.decode_block() {
            Ok(block) => block,
            Err(error) => {
                tracing::error!(%error, "failed to decode block");
                return;
            }
        };
        let header = BlockHeader::from(&block.header);
        let point = header.point();
        tracing::debug!(%point, "received block");

        // check that body belongs to header
        if header.header().header_body.block_body_hash != block.body_hash() {
            tracing::warn!(expected = %header.header().header_body.block_body_hash, actual = %block.body_hash(), "block body hash mismatch");
            return;
        }
        let Some(range) = self.missing_range.as_ref() else {
            tracing::warn!("received block with no outstanding missing range");
            return;
        };
        if header.parent_hash() != Some(range.boundary().hash()) {
            tracing::warn!(expected = ?Some(range.boundary().hash()), actual = ?header.parent_hash(), "block parent hash mismatch");
            return;
        }
        if Some(point) != range.first() {
            tracing::warn!(expected = ?range.first(), actual = ?point, "block point mismatch");
            return;
        }

        store
            .store_block(&point.hash(), &network_block.raw_block())
            .await
            .or_terminate(&eff, async |error| {
                tracing::error!(%error, "failed to store block");
            })
            .await;
        let tip = Tip::new(point, block.header.header_body.block_number.into());
        eff.send(&self.downstream, (tip, range.boundary(), self.block_height)).await;

        let done = {
            #[expect(clippy::expect_used)]
            let range = self.missing_range.as_mut().expect("checked above that this exists");
            range.shift_one_block();
            range.is_empty()
        };
        if done {
            self.missing_range = None;
            if let Some(timeout) = self.timeout.take() {
                eff.cancel_schedule(timeout).await;
            }
            eff.send(&self.upstream, SelectChainMsg::FetchNextFrom(point)).await;
        }
    }

    pub async fn timeout(&mut self, req_id: u64, eff: Effects<FetchBlocksMsg>) {
        if req_id != self.req_id || self.missing_range.is_none() {
            return;
        }
        tracing::error!(%req_id, "timeout fetching blocks");
        self.timeout = None;
        #[expect(clippy::expect_used)]
        let from = self.missing_range.take().expect("checked above that this exists").boundary();
        eff.send(&self.upstream, SelectChainMsg::FetchNextFrom(from)).await;
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum FetchBlocksMsg {
    NewTip(Tip, Point),
    Block(NetworkBlock),
    Timeout(u64),
}

pub async fn stage(mut state: FetchBlocks, msg: FetchBlocksMsg, eff: Effects<FetchBlocksMsg>) -> FetchBlocks {
    if state.cleanup_replies.is_blackhole() {
        let stage = eff.stage("cleanup_replies", cleanup_replies).await;
        state.cleanup_replies = eff.wire_up(stage, (0, eff.me())).await;
    }
    match msg {
        FetchBlocksMsg::NewTip(tip, parent) => state.new_tip(tip, parent, eff).await,
        FetchBlocksMsg::Block(block) => state.block(block, eff).await,
        FetchBlocksMsg::Timeout(req_id) => state.timeout(req_id, eff).await,
    }
    state
}

/// Ensure that straggling block replies do not clog the mailbox of the fetch stage.
pub async fn cleanup_replies(
    (curr_id, fetch): (u64, StageRef<FetchBlocksMsg>),
    msg: Blocks2,
    eff: Effects<Blocks2>,
) -> (u64, StageRef<FetchBlocksMsg>) {
    match msg {
        // completely ignore empty responses, fetch stage will deal with timeouts
        Blocks2::NoBlocks(_) => (curr_id, fetch),
        // ignore responses to prior requests
        Blocks2::Block(id, _) if id < curr_id => (curr_id, fetch),
        Blocks2::Block(id, block) => {
            eff.send(&fetch, FetchBlocksMsg::Block(block)).await;
            // getting higher id implies a new request has started
            (id.max(curr_id), fetch)
        }
        // getting done message implies a new request will start with id+1, but Done might be old as well
        Blocks2::Done(id) => ((id + 1).max(curr_id), fetch),
    }
}

#[cfg(test)]
mod test_setup;
#[cfg(test)]
mod tests;
