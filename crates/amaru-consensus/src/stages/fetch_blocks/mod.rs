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

use amaru_kernel::{BlockHeader, BlockHeight, IsHeader, Peer, Point, Tip, cardano::network_block::NetworkBlock};
use amaru_ouroboros::{ChainStore, ReadOnlyChainStore};
use amaru_protocols::{blockfetch::Blocks2, manager::ManagerMessage, store_effects::Store};
use pure_stage::{Effects, ScheduleId, StageRef, TryInStage};

use crate::stages::{block_source::BlockSourceMsg, select_chain::SelectChainMsg};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FetchBlocks {
    downstream: StageRef<(Tip, Point, BlockHeight)>,
    req_id: u64,
    missing: Vec<Point>,
    upstream: StageRef<SelectChainMsg>,
    manager: StageRef<ManagerMessage>,
    block_source: StageRef<BlockSourceMsg>,
    cleanup_replies: StageRef<Blocks2>,
    timeout: Option<ScheduleId>,
    block_height: BlockHeight,
}

impl FetchBlocks {
    pub fn new(
        downstream: StageRef<(Tip, Point, BlockHeight)>,
        upstream: StageRef<SelectChainMsg>,
        manager: StageRef<ManagerMessage>,
        block_source: StageRef<BlockSourceMsg>,
    ) -> Self {
        Self {
            downstream,
            req_id: 0,
            missing: Vec::new(),
            upstream,
            manager,
            block_source,
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
        block_source: StageRef<BlockSourceMsg>,
        cleanup_replies: StageRef<Blocks2>,
    ) -> Self {
        Self {
            downstream,
            req_id: 0,
            missing: Vec::new(),
            upstream,
            manager,
            block_source,
            cleanup_replies,
            timeout: None,
            block_height: BlockHeight::from(0),
        }
    }

    #[expect(clippy::expect_used)]
    pub async fn new_tip(&mut self, tip: Tip, parent: Point, eff: Effects<FetchBlocksMsg>) {
        self.block_height = tip.block_height().max(self.block_height);

        tracing::debug!(tip = %tip.point(), parent = %parent, "fetching blocks");
        let store = Store::new(eff);
        // find blocks to retrieve
        assert!(self.missing.is_empty(), "missing blocks should be empty when starting a new tip: {:?}", self.missing);
        let initial = store
            .load_header(&tip.hash())
            .or_terminate(store.eff(), async |_| {
                tracing::error!("failed to load initial header");
            })
            .await;
        let mut failed_hash = None;
        // don't fetch the anchor block because that will confuse block validation
        let anchor = store.get_anchor_hash();
        for header in store.ancestors(initial) {
            let Ok(block) = store.load_block(&header.hash()) else {
                failed_hash = Some(header.hash());
                break;
            };
            if block.is_some() || header.hash() == anchor {
                // push the parent of the first block to fetch
                self.missing.push(header.point());
                break;
            }
            self.missing.push(header.point());
        }
        if let Some(failed_hash) = failed_hash {
            tracing::error!(hash = %failed_hash, "failed to load block");
            return store.eff().terminate().await;
        }
        self.missing.reverse();
        // TODO make configurable
        self.missing.truncate(25);
        // only the first parent is not enough
        if self.missing.len() < 2 {
            self.missing.clear();
            tracing::info!(tip = %tip.point(), parent = %parent, "no blocks to fetch");
            store.eff().send(&self.upstream, SelectChainMsg::FetchNextFrom(tip.point())).await;
            return;
        }
        // request blocks
        let from = *self.missing.get(1).expect("checked above that this exists");
        let through = *self.missing.last().expect("checked above that not empty");
        tracing::debug!(%from, %through, length = self.missing.len() - 1, "requesting blocks");
        self.req_id += 1;
        store
            .eff()
            .send(
                &self.manager,
                ManagerMessage::FetchBlocks2 { from, through, id: self.req_id, cr: self.cleanup_replies.clone() },
            )
            .await;
        let timeout = store.eff().schedule_after(FetchBlocksMsg::Timeout(self.req_id), Duration::from_secs(5)).await;
        self.timeout = Some(timeout);
    }

    pub async fn block(&mut self, peer: Peer, network_block: NetworkBlock, eff: Effects<FetchBlocksMsg>) {
        let store = Store::new(eff);
        let block = match network_block.decode_block() {
            Ok(block) => block,
            Err(error) => {
                tracing::error!(%error, "failed to decode block");
                return;
            }
        };
        let header = BlockHeader::from(&block.header);
        let point = header.point();
        let block_height = block.header.header_body.block_number.into();
        store.eff().send(&self.block_source, BlockSourceMsg::BlockReceived { peer, point, block_height }).await;
        tracing::debug!(%point, "received block");

        // check that body belongs to header
        if header.header().header_body.block_body_hash != block.body_hash() {
            tracing::warn!(expected = %header.header().header_body.block_body_hash, actual = %block.body_hash(), "block body hash mismatch");
            return;
        }
        // check that parent is as expected
        if header.parent_hash() != self.missing.first().map(|p| p.hash()) {
            tracing::warn!(expected = ?self.missing.first().map(|p| p.hash()), actual = ?header.parent_hash(), "block parent hash mismatch");
            return;
        }
        // check that block's point is as expected
        if Some(&point) != self.missing.get(1) {
            tracing::warn!(expected = ?self.missing.get(1), actual = ?point, "block point mismatch");
            return;
        }

        let parent = self.missing.remove(0);
        store
            .store_block(&point.hash(), &network_block.raw_block())
            .or_terminate(store.eff(), async |error| {
                tracing::error!(%error, "failed to store block");
            })
            .await;
        let tip = Tip::new(point, block.header.header_body.block_number.into());
        store.eff().send(&self.downstream, (tip, parent, self.block_height)).await;

        // check if we are done
        if self.missing.len() < 2 {
            self.missing.clear();
            if let Some(timeout) = self.timeout.take() {
                store.eff().cancel_schedule(timeout).await;
            }
            store.eff().send(&self.upstream, SelectChainMsg::FetchNextFrom(point)).await;
        }
    }

    pub async fn timeout(&mut self, req_id: u64, eff: Effects<FetchBlocksMsg>) {
        if req_id != self.req_id || self.missing.is_empty() {
            return;
        }
        tracing::error!(%req_id, "timeout fetching blocks");
        self.timeout = None;
        let from = self.missing.remove(0);
        self.missing.clear();
        eff.send(&self.upstream, SelectChainMsg::FetchNextFrom(from)).await;
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum FetchBlocksMsg {
    NewTip(Tip, Point),
    Block(Peer, NetworkBlock),
    Timeout(u64),
}

pub async fn stage(mut state: FetchBlocks, msg: FetchBlocksMsg, eff: Effects<FetchBlocksMsg>) -> FetchBlocks {
    if state.cleanup_replies.is_blackhole() {
        let stage = eff.stage("cleanup_replies", cleanup_replies).await;
        state.cleanup_replies = eff.wire_up(stage, (0, eff.me())).await;
    }
    match msg {
        FetchBlocksMsg::NewTip(tip, parent) => state.new_tip(tip, parent, eff).await,
        FetchBlocksMsg::Block(peer, block) => state.block(peer, block, eff).await,
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
        Blocks2::Block(id, _, _) if id < curr_id => (curr_id, fetch),
        Blocks2::Block(id, peer, block) => {
            eff.send(&fetch, FetchBlocksMsg::Block(peer, block)).await;
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
