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

use crate::stages::peer_selection::PeerSelectionMsg;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BlockSource {
    adopted_tip: Tip,
    max_tip_distance: u64,
    by_point: BTreeMap<Point, BlockValidity>,
    invalid_peer_sink: StageRef<PeerSelectionMsg>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum BlockValidity {
    Pending(BlockHeight, BTreeSet<Peer>),
    Valid(BlockHeight),
    Invalid(BlockHeight),
}

impl BlockValidity {
    fn block_height(&self) -> BlockHeight {
        match self {
            BlockValidity::Pending(h, _) => *h,
            BlockValidity::Valid(h) => *h,
            BlockValidity::Invalid(h) => *h,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum BlockSourceMsg {
    BlockReceived { peer: Peer, point: Point, block_height: BlockHeight },
    Validation { valid: bool, point: Point },
    AdoptedTip(Tip),
}

impl BlockSource {
    pub fn new(adopted_tip: Tip, max_tip_distance: u64, invalid_peer_sink: StageRef<PeerSelectionMsg>) -> Self {
        Self { adopted_tip, max_tip_distance, by_point: BTreeMap::new(), invalid_peer_sink }
    }

    fn prune(&mut self) {
        let span = tracing::debug_span!("block_source.prune", pruned = field::Empty, retained = field::Empty).entered();
        let adopted_h = self.adopted_tip.block_height();
        let entries = self.by_point.len();
        self.by_point.retain(|_, entry| entry.block_height() - self.max_tip_distance <= adopted_h);
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
        use BlockValidity::*;

        tracing::debug!(%peer, %point, %block_height, "block received");
        match self.by_point.get_mut(&point) {
            Some(Invalid(_height)) => {
                tracing::info!(%peer, %point, "received known invalid block from new peer");
                eff.send(&self.invalid_peer_sink, PeerSelectionMsg::Adversarial(peer)).await;
            }
            Some(Pending(_height, peers)) => {
                peers.insert(peer);
            }
            Some(Valid(_height)) => {
                // do nothing
            }
            None => {
                self.by_point.insert(point, Pending(block_height, BTreeSet::from([peer])));
            }
        }
        self.prune();
    }

    async fn on_validation(&mut self, valid: bool, point: Point, eff: &Effects<BlockSourceMsg>) {
        tracing::debug!(%valid, %point, "validation result");
        if let Some(validity) = self.by_point.get_mut(&point)
            && let BlockValidity::Pending(height, peers) = validity
        {
            if valid {
                *validity = BlockValidity::Valid(*height);
            } else {
                for p in std::mem::take(peers) {
                    eff.send(&self.invalid_peer_sink, PeerSelectionMsg::Adversarial(p.clone())).await;
                }
                *validity = BlockValidity::Invalid(*height);
            }
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
