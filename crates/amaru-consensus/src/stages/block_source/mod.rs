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

use amaru_kernel::{BlockHeight, Peer, Point};
use amaru_observability::{debug, debug_record, debug_span, info};
use amaru_pure_stage::{Effects, StageRef};

use crate::stages::peer_selection::PeerSelectionMsg;

/// Tracks provenance of blocks received from the network (by `Point`) and
/// reports peers that provided invalid blocks as adversarial.
///
/// This stage is a leaf in the amaru_pure_stage graph. It receives *notifications*
/// (not requests) and performs no serving of blocks/headers, no downstream
/// block emission, and no interaction with any manager or chain-selection
/// logic. Its only external effect is sending `PeerSelectionMsg::Adversarial`
/// to the `invalid_peer_sink` (a `StageRef` to the peer_selection stage,
/// supplied at construction).
///
/// ## State
/// - `adopted_tip: Point`: The node's current adopted tip. Updated only via
///   `AdoptedTip` messages. Serves as the base for pruning.
/// - `max_tip_distance: u64`: Window size (in block heights). Used **solely**
///   by `prune()` to discard tracking entries whose height is < `adopted_h - max_tip_distance`.
///   Has no effect on any serving behavior (none exists here).
/// - `by_point: BTreeMap<Point, BlockValidity>`: Core tracking map.
///   - `Pending(h, peers)`: Block at this point has been announced by one or
///     more peers; awaiting `Validation`.
///   - `Valid(h)`: Block validated successfully. Further `BlockReceived` for
///     the same point are ignored.
///   - `Invalid(h)`: Block was invalid. Any subsequent `BlockReceived` (even
///     from repeat peers) immediately faults the sender.
/// - `invalid_peer_sink: StageRef<PeerSelectionMsg>`: The only place this
///   stage ever sends.
///
/// ## Message Handling (`stage()` entry point)
/// All paths call `prune()` (except pure `AdoptedTip`, which does it directly).
///
/// - `BlockSourceMsg::BlockReceived { peer, tip }`:
///   - If entry is `Invalid(_)`: log and **immediately** send `Adversarial(peer)`.
///   - If `Pending(_, peers)`: insert the peer into the set (dedup).
///   - If `Valid(_)`: no-op.
///   - If absent: insert `Pending(height, {peer})`.
///   - Then `prune()`.
///
/// - `BlockSourceMsg::Validation { valid, point }`:
///   - Only acts if a `Pending(height, peers)` exists for the point (otherwise no-op).
///   - If `valid`: transition to `Valid(height)`.
///   - If `!valid`: send `Adversarial(p)` for every peer in the set, then set `Invalid(height)`.
///   - Then `prune()`.
///   - **Ordering assumption**: `Validation` is expected after the corresponding `BlockReceived`(s).
///
/// - `BlockSourceMsg::AdoptedTip(tip)`:
///   - `self.adopted_tip = tip; prune();` (no sends).
///
/// ## Pruning
/// `prune()` retains only entries where `entry.block_height() >= adopted_h - max_tip_distance`.
/// Called after every `BlockReceived`/`Validation` and on `AdoptedTip`.
///
/// Construction: `BlockSource::new(adopted_tip, max_tip_distance, invalid_peer_sink)`.
/// The `stage()` function is the amaru_pure_stage handler.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BlockSource {
    adopted_tip: Point,
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
    BlockReceived { peer: Peer, tip: Point },
    Validation { valid: bool, point: Point },
    AdoptedTip(Point),
}

impl BlockSource {
    pub fn new(adopted_tip: Point, max_tip_distance: u64, invalid_peer_sink: StageRef<PeerSelectionMsg>) -> Self {
        Self { adopted_tip, max_tip_distance, by_point: BTreeMap::new(), invalid_peer_sink }
    }

    fn prune(&mut self) {
        let _span = debug_span!(consensus::block_source::PRUNE).entered();
        let adopted_h = self.adopted_tip.block_height();
        let entries = self.by_point.len();
        self.by_point.retain(|_, entry| entry.block_height() >= adopted_h - self.max_tip_distance);
        let retained = self.by_point.len();
        debug_record!(consensus::block_source::PRUNE, pruned = entries - retained, retained);
    }

    async fn on_block_received(&mut self, peer: Peer, point: Point, eff: &Effects<BlockSourceMsg>) {
        use BlockValidity::*;

        debug!(consensus::block_source::RECEIVED, peer, point);
        match self.by_point.get_mut(&point) {
            Some(Invalid(_height)) => {
                info!(consensus::block_source::KNOWN_INVALID, peer, point);
                eff.send(&self.invalid_peer_sink, PeerSelectionMsg::adversarial(peer)).await;
            }
            Some(Pending(_height, peers)) => {
                peers.insert(peer);
            }
            Some(Valid(_height)) => {
                // do nothing
            }
            None => {
                self.by_point.insert(point, Pending(point.block_height(), BTreeSet::from([peer])));
            }
        }
        self.prune();
    }

    async fn on_validation(&mut self, valid: bool, point: Point, eff: &Effects<BlockSourceMsg>) {
        debug!(consensus::block_source::VALIDATION, point, valid);
        if let Some(validity) = self.by_point.get_mut(&point)
            && let BlockValidity::Pending(height, peers) = validity
        {
            if valid {
                *validity = BlockValidity::Valid(*height);
            } else {
                for p in std::mem::take(peers) {
                    eff.send(&self.invalid_peer_sink, PeerSelectionMsg::adversarial(p.clone())).await;
                }
                *validity = BlockValidity::Invalid(*height);
            }
        }
        self.prune();
    }

    fn on_adopted_tip(&mut self, tip: Point) {
        self.adopted_tip = tip;
        self.prune();
    }
}

/// The block source stage receives notifications of blocks received from the
/// network, and their validation results.
///
/// It tracks which peers have sent which blocks, and if a block is deemed invalid,
/// it reports all peers that sent that block as adversarial to the peer selection stage.
pub async fn stage(mut state: BlockSource, msg: BlockSourceMsg, eff: Effects<BlockSourceMsg>) -> BlockSource {
    match msg {
        BlockSourceMsg::BlockReceived { peer, tip } => {
            state.on_block_received(peer, tip, &eff).await;
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
