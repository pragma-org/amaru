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

use std::collections::{BTreeMap, BTreeSet};

use amaru_kernel::{BlockHeight, Peer, Point, Tip};
use pure_stage::{Effects, StageRef};
use tracing::field;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BlockSource {
    adopted_tip: Tip,
    max_tip_distance: u64,
    by_point: BTreeMap<Point, BlockSourceEntry>,
    invalid_peer_sink: StageRef<BlockSourceFault>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) enum BlockValidity {
    Pending,
    Valid,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(super) struct BlockSourceEntry {
    peers: BTreeSet<Peer>,
    block_height: BlockHeight,
    validity: BlockValidity,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum BlockSourceMsg {
    BlockReceived { peer: Peer, point: Point, block_height: BlockHeight },
    Validation { valid: bool, point: Point },
    AdoptedTip(Tip),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum BlockSourceFault {
    InvalidBlock { peer: Peer, point: Point },
}

impl BlockSource {
    pub fn new(adopted_tip: Tip, max_tip_distance: u64, invalid_peer_sink: StageRef<BlockSourceFault>) -> Self {
        Self { adopted_tip, max_tip_distance, by_point: BTreeMap::new(), invalid_peer_sink }
    }

    fn prune(&mut self) {
        let span = tracing::debug_span!("block_source.prune", pruned = field::Empty, retained = field::Empty).entered();
        let adopted_h = self.adopted_tip.block_height().as_u64();
        let entries = self.by_point.len();
        self.by_point.retain(|_, entry| {
            let h = entry.block_height.as_u64();
            adopted_h.saturating_sub(h) <= self.max_tip_distance
        });
        let retained = self.by_point.len();
        span.record("pruned", entries - retained);
        span.record("retained", retained);
    }

    async fn on_block_received(
        &mut self,
        peer: Peer,
        point: Point,
        block_height: BlockHeight,
        eff: &Effects<BlockSourceMsg>,
    ) {
        tracing::debug!(%peer, %point, %block_height, "block received");
        match self.by_point.get_mut(&point) {
            Some(entry) => {
                entry.block_height = block_height;
                match entry.validity {
                    BlockValidity::Invalid => {
                        if entry.peers.insert(peer.clone()) {
                            tracing::info!(%peer, %point, "received known invalid block from new peer");
                            eff.send(&self.invalid_peer_sink, BlockSourceFault::InvalidBlock { peer, point }).await;
                        }
                    }
                    BlockValidity::Pending | BlockValidity::Valid => {
                        entry.peers.insert(peer);
                    }
                }
            }
            None => {
                self.by_point.insert(
                    point,
                    BlockSourceEntry { peers: BTreeSet::from([peer]), block_height, validity: BlockValidity::Pending },
                );
            }
        }
        self.prune();
    }

    async fn on_validation(&mut self, valid: bool, point: Point, eff: &Effects<BlockSourceMsg>) {
        tracing::debug!(%valid, %point, "validation result");
        if valid {
            if let Some(entry) = self.by_point.get_mut(&point) {
                entry.validity = BlockValidity::Valid;
            }
        } else if let Some(entry) = self.by_point.get_mut(&point) {
            for p in entry.peers.iter() {
                eff.send(&self.invalid_peer_sink, BlockSourceFault::InvalidBlock { peer: p.clone(), point }).await;
            }
            entry.validity = BlockValidity::Invalid;
        }
        self.prune();
    }

    fn on_adopted_tip(&mut self, tip: Tip) {
        self.adopted_tip = tip;
        self.prune();
    }
}

pub async fn stage(mut state: BlockSource, msg: BlockSourceMsg, eff: Effects<BlockSourceMsg>) -> BlockSource {
    match msg {
        BlockSourceMsg::BlockReceived { peer, point, block_height } => {
            state.on_block_received(peer, point, block_height, &eff).await;
        }
        BlockSourceMsg::Validation { valid, point } => {
            state.on_validation(valid, point, &eff).await;
        }
        BlockSourceMsg::AdoptedTip(tip) => {
            state.on_adopted_tip(tip);
        }
    }
    state
}

#[cfg(test)]
mod test_setup;
#[cfg(test)]
mod tests;
