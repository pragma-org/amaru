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

use amaru_kernel::{BlockHeader, IsHeader, Point, Tip, cardano::network_block::NetworkBlock};
use amaru_ouroboros::{ChainStore, ReadOnlyChainStore};
use amaru_protocols::{blockfetch::Blocks2, manager::ManagerMessage, store_effects::Store};
use pure_stage::{Effects, ScheduleId, StageRef, TryInStage};

use crate::stages::select_chain_new::SelectChainMsg;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FetchBlocks {
    downstream: StageRef<(Tip, Point)>,
    req_id: u64,
    from: Point,
    through: Point,
    current: Point,
    upstream: StageRef<SelectChainMsg>,
    manager: StageRef<ManagerMessage>,
    cleanup_replies: StageRef<Blocks2>,
    timeout: Option<ScheduleId>,
}

impl FetchBlocks {
    pub fn new(
        downstream: StageRef<(Tip, Point)>,
        upstream: StageRef<SelectChainMsg>,
        manager: StageRef<ManagerMessage>,
    ) -> Self {
        Self {
            downstream,
            req_id: 0,
            from: Point::Origin,
            through: Point::Origin,
            current: Point::Origin,
            upstream,
            manager,
            cleanup_replies: StageRef::blackhole(),
            timeout: None,
        }
    }

    /// Constructor for tests: use a mock cleanup_replies stage instead of wiring the real one.
    #[cfg(test)]
    pub fn for_tests(
        downstream: StageRef<(Tip, Point)>,
        upstream: StageRef<SelectChainMsg>,
        manager: StageRef<ManagerMessage>,
        cleanup_replies: StageRef<Blocks2>,
    ) -> Self {
        Self {
            downstream,
            req_id: 0,
            from: Point::Origin,
            through: Point::Origin,
            current: Point::Origin,
            upstream,
            manager,
            cleanup_replies,
            timeout: None,
        }
    }

    pub async fn new_tip(&mut self, tip: Tip, parent: Point, eff: Effects<FetchBlocksMsg>) {
        tracing::debug!(tip = %tip.point(), parent = %parent, "fetching blocks");
        let store = Store::new(eff);
        // find blocks to retrieve
        let mut missing = Vec::new();
        let initial = store
            .load_header(&tip.hash())
            .or_terminate(store.eff(), async |_| {
                tracing::error!("failed to load initial header");
            })
            .await;
        let mut failed_hash = None;
        let anchor = store.get_anchor_hash();
        // don't fetch the anchor block because that will confuse block validation
        for header in store.ancestors(initial).filter(|h| h.hash() != anchor) {
            let Ok(block) = store.load_block(&header.hash()) else {
                failed_hash = Some(header.hash());
                break;
            };
            if block.is_some() {
                break;
            }
            missing.push(header.point());
        }
        if let Some(failed_hash) = failed_hash {
            tracing::error!(hash = %failed_hash, "failed to load block");
            return store.eff().terminate().await;
        }
        missing.reverse();
        // TODO make configurable
        missing.truncate(10);
        if missing.is_empty() {
            tracing::info!(tip = %tip.point(), parent = %parent, "no blocks to fetch");
            store.eff().send(&self.upstream, SelectChainMsg::FetchNextFrom(tip.point())).await;
            return;
        }
        // request blocks
        #[expect(clippy::expect_used)]
        {
            self.from = *missing.first().expect("checked above that not empty");
            self.through = *missing.last().expect("checked above that not empty");
        }
        tracing::info!(from = %self.from, through = %self.through, length = %missing.len(), "requesting blocks");
        self.current = Point::Origin;
        self.req_id += 1;
        store
            .eff()
            .send(
                &self.manager,
                ManagerMessage::FetchBlocks2 {
                    from: self.from,
                    through: self.through,
                    id: self.req_id,
                    cr: self.cleanup_replies.clone(),
                },
            )
            .await;
        let timeout = store.eff().schedule_after(FetchBlocksMsg::Timeout(self.req_id), Duration::from_secs(5)).await;
        self.timeout = Some(timeout);
    }

    pub async fn block(&mut self, network_block: NetworkBlock, eff: Effects<FetchBlocksMsg>) {
        let store = Store::new(eff);
        let block = network_block
            .decode_block()
            .or_terminate(store.eff(), async |error| {
                tracing::error!(%error, "failed to decode block");
            })
            .await;
        let header = BlockHeader::from(&block.header);
        let point = header.point();
        tracing::debug!(%point, "received block");
        let accept = if point == self.from && self.current == Point::Origin {
            self.current = point;
            let anchor = {
                #[expect(clippy::expect_used)]
                store.load_tip(&store.get_anchor_hash()).expect("anchor hash not found").point()
            };
            let parent = if point > anchor
                && let Some(h) = header.parent_hash()
            {
                tracing::debug!(anchor = %anchor, parent = %h, "loading parent");
                store
                    .load_tip(&h)
                    .or_terminate(store.eff(), async |_| {
                        tracing::error!(anchor = %anchor, parent = %h, %point, "failed to load parent of block");
                    })
                    .await
                    .point()
            } else {
                tracing::error!(anchor = %anchor, %point, "no parent of block, which is needed by the ledger");
                return store.eff().terminate().await;
            };
            Some(parent)
        } else if header.parent_hash() == Some(self.current.hash()) {
            let parent = self.current;
            self.current = point;
            Some(parent)
        } else {
            None
        };
        if let Some(parent) = accept {
            store
                .store_block(&point.hash(), &network_block.raw_block())
                .or_terminate(store.eff(), async |error| {
                    tracing::error!(%error, "failed to store block");
                })
                .await;
            let tip = Tip::new(point, block.header.header_body.block_number.into());
            store.eff().send(&self.downstream, (tip, parent)).await;
            if point == self.through {
                if let Some(timeout) = self.timeout.take() {
                    store.eff().cancel_schedule(timeout).await;
                }
                store.eff().send(&self.upstream, SelectChainMsg::FetchNextFrom(point)).await;
            }
        }
    }

    pub async fn timeout(&mut self, req_id: u64, eff: Effects<FetchBlocksMsg>) {
        if req_id != self.req_id {
            return;
        }
        tracing::error!(%req_id, "timeout fetching blocks");
        self.timeout = None;
        eff.send(&self.upstream, SelectChainMsg::FetchNextFrom(self.from)).await;
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
