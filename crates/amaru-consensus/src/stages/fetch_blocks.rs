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

use amaru_kernel::{Block, BlockHeader, IsHeader, Point, RawBlock, Tip, to_cbor};
use amaru_ouroboros::{ChainStore, ReadOnlyChainStore};
use amaru_protocols::{blockfetch::Blocks2, manager::ManagerMessage, store_effects::Store};
use pure_stage::{Effects, StageRef, TryInStage};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FetchBlocks {
    downstream: StageRef<Tip>,
    req_id: u64,
    from: Point,
    through: Point,
    current: Point,
    upstream: StageRef<SendNext>,
    manager: StageRef<ManagerMessage>,
    cleanup_replies: StageRef<Blocks2>,
}

impl FetchBlocks {
    pub fn new(downstream: StageRef<Tip>, upstream: StageRef<SendNext>, manager: StageRef<ManagerMessage>) -> Self {
        Self {
            downstream,
            req_id: 0,
            from: Point::Origin,
            through: Point::Origin,
            current: Point::Origin,
            upstream,
            manager,
            cleanup_replies: StageRef::blackhole(),
        }
    }

    pub async fn new_tip(&mut self, tip: Tip, eff: Effects<FetchBlocksMsg>) {
        let store = Store::new(eff);
        // find blocks to retrieve
        let mut missing = Vec::new();
        for (header, valid) in store.ancestors_with_validity(tip.hash()) {
            if valid.is_some() {
                break;
            }
            missing.push(header.point());
        }
        missing.reverse();
        // TODO make configurable
        missing.truncate(10);
        if missing.is_empty() {
            // send rollback
            store.eff().send(&self.downstream, tip).await;
            store.eff().send(&self.upstream, SendNext(tip.point())).await;
            return;
        }
        // request blocks
        self.from = *missing.first().unwrap();
        self.through = *missing.last().unwrap();
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
    }

    pub async fn block(&mut self, block: Block, eff: Effects<FetchBlocksMsg>) {
        let store = Store::new(eff);
        let header = BlockHeader::from(&block.header);
        let point = header.point();
        let accept = if point == self.from && self.current == Point::Origin {
            self.current = point;
            true
        } else if header.parent_hash() == Some(self.current.hash()) {
            self.current = point;
            true
        } else {
            false
        };
        if accept {
            store
                .store_block(&point.hash(), &RawBlock::from(to_cbor(&block).as_slice()))
                .or_terminate(store.eff(), async |error| {
                    tracing::error!(%error, "failed to store block");
                })
                .await;
            store.eff().send(&self.downstream, Tip::new(point, block.header.header_body.block_number.into())).await;
            if point == self.through {
                store.eff().send(&self.upstream, SendNext(point)).await;
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SendNext(Point);

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum FetchBlocksMsg {
    NewTip(Tip),
    Block(Block),
}

pub async fn stage(mut state: FetchBlocks, msg: FetchBlocksMsg, eff: Effects<FetchBlocksMsg>) -> FetchBlocks {
    if state.cleanup_replies.is_blackhole() {
        let stage = eff.stage("cleanup_replies", cleanup_replies).await;
        state.cleanup_replies = eff.wire_up(stage, (0, eff.me())).await;
    }
    match msg {
        FetchBlocksMsg::NewTip(tip) => state.new_tip(tip, eff).await,
        FetchBlocksMsg::Block(block) => state.block(block, eff).await,
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
