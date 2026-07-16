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

use std::{cmp::Ordering, time::Duration};

use amaru_kernel::{BlockHeader, BlockHeight, IsHeader, Point, Tip};
use amaru_ouroboros::MempoolMsg;
use amaru_ouroboros_traits::{FindAncestorOnBestChainResult, StoreError};
use amaru_protocols::{manager::ManagerMessage, store_effects::Store};
use amaru_pure_stage::{Effects, Instant, OrTerminateWith, StageRef};

use crate::stages::{block_source::BlockSourceMsg, select_chain::cmp_tip};

/// The AdoptChain stage decides whether (and how) to adopt a newly validated
/// chain tip as the node's new best chain, updating the persistent chain store
/// view and notifying dependent stages.
///
/// It is a leaf stage in the consensus pipeline (receives `AdoptChainMsg`
/// containing a `Tip` + `max_block_height` hint, typically produced by
/// `validate_block`). It only adopts when the candidate is *strictly better*
/// than the current best (height first, then `cmp_tip` on loaded `BlockHeader`s,
/// which breaks ties via op-cert sequence number among other factors).
///
/// On adoption:
/// - For direct extensions (parent == current best hash): `roll_forward_chain`.
/// - For forks: `find_ancestor_on_best_chain` (to compute the rollback point
///   as the youngest ancestor on the prior best chain) followed by
///   `switch_to_fork`.
/// - Always: `drag_anchor_forward` (anchor moved forward at most
///   `consensus_security_param` ("k") blocks behind the new tip; the height
///   walk uses a consistent snapshot effect; only the write is separate;
///   never moves backward; may be a no-op if no suitable anchor found).
/// - Then (after logging with 1s suppression for catch-up): three `eff.send`s
///   (in this exact order):
///   1. `MempoolMsg::NewTip(tip)` — to the mempool (to flush invalidated txs
///      and adjust to the new tip).
///   2. `ManagerMessage::NewTip(tip)` — to the downstream manager/peer layer.
///   3. `BlockSourceMsg::AdoptedTip(tip)` — to block_source (so it can adjust
///      its tip distance / fetch window for the newly adopted chain).
/// - Finally, `current_best_tip` is updated in the returned state.
///
/// Early exits (no adoption, no sends, no anchor drag, no logging of "adopted"):
/// - Incoming tip height < current best height (debug log + return).
/// - After loading headers: `cmp_tip` says incoming is not strictly greater
///   (debug log + return).
///
/// Error/termination paths (all via `.or_terminate_with`, producing Voluntary
/// termination + specific warn/error logs):
/// - Failed `load_header` for the incoming tip ("failed to load incoming tip").
/// - Failed `load_header` for the prior current best ("failed to load current best").
/// - Any `StoreError` from `adopt_tip` (inside: `roll_forward_chain`,
///   `find_ancestor_on_best_chain`, or `switch_to_fork`; "failed to adopt tip").
/// - `roll_forward_chain` in the origin/first-tip case ("failed to adopt first tip").
/// - `drag_anchor_forward` errors ("failed to drag anchor forward").
///
/// `max_block_height` is unconditionally updated (`.max`) on every message
/// (even skipped ones) and is only surfaced in adoption logging.
/// The stage holds `StageRef`s to exactly three external actors (downstream
/// manager, block_source, mempool) and the k parameter; it never spawns
/// children or wires additional stages. Initial `current_best_tip` is
/// supplied at construction (commonly `Tip::origin()`); the in-memory copy
/// is the source of truth for quick height guards and is kept in sync with
/// the store only on successful adoption paths.
///
/// The stage is tested exclusively via amaru_pure_stage simulation (see
/// `test_setup.rs`): `HeaderTree` + `InMemConsensusStore` + `ResourceHeaderStore`,
/// exhaustive `te_*` effect tracers for every `LoadHeaderEffect`,
/// `RollForwardChainEffect`, `SwitchToForkEffect`, `FindAncestorOnBestChainEffect`,
/// `FindAnchorAtHeightEffect`, `SetAnchorHashEffect`, `clock`, `send`, etc.,
/// plus `assert_trace` + post-run store assertions + log scrubbing.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AdoptChain {
    downstream: StageRef<ManagerMessage>,
    block_source: StageRef<BlockSourceMsg>,
    mempool: StageRef<MempoolMsg>,
    consensus_security_param: u64,
    current_best_tip: Tip,
    max_block_height: BlockHeight,
    last_printed: Instant,
    suppressed: u32,
}

impl AdoptChain {
    pub fn new(
        downstream: StageRef<ManagerMessage>,
        block_source: StageRef<BlockSourceMsg>,
        mempool: StageRef<MempoolMsg>,
        consensus_security_param: u64,
        current_best_tip: Tip,
    ) -> Self {
        Self {
            downstream,
            block_source,
            mempool,
            consensus_security_param,
            current_best_tip,
            max_block_height: BlockHeight::from(0),
            last_printed: Instant::at_offset(Duration::ZERO, Duration::ZERO),
            suppressed: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AdoptChainMsg {
    tip: Tip,
    max_block_height: BlockHeight,
}

impl AdoptChainMsg {
    pub fn new(tip: Tip, max_block_height: BlockHeight) -> Self {
        Self { tip, max_block_height }
    }
}

pub async fn stage(mut state: AdoptChain, msg: AdoptChainMsg, eff: Effects<AdoptChainMsg>) -> AdoptChain {
    state.max_block_height = msg.max_block_height.max(state.max_block_height);
    let AdoptChainMsg { tip: msg, .. } = msg;

    if msg.block_height() < state.current_best_tip.block_height() {
        tracing::debug!(tip = %msg, current_best_tip = %state.current_best_tip, "incoming tip shorter than current best, skipping");
        return state;
    }

    let store = Store::new(eff.clone());

    let incoming_header = store
        .load_header(&msg.hash())
        .or_terminate_with(&eff, async |_| {
            tracing::warn!(tip = %msg, "failed to load incoming tip");
        })
        .await;

    let current_best = if state.current_best_tip.point() == Point::Origin {
        None
    } else {
        Some(
            store
                .load_header(&state.current_best_tip.hash())
                .or_terminate_with(&eff, async |_| {
                    tracing::warn!("failed to load current best");
                })
                .await,
        )
    };

    if cmp_tip(Some(&incoming_header), current_best.as_ref()) != Ordering::Greater {
        tracing::debug!(tip = %msg, "incoming tip not better than current best, not adopting");
        return state;
    }

    if let Some(current_best) = current_best.as_ref() {
        let adopt_result = adopt_tip(&store, &incoming_header, current_best)
            .or_terminate_with(&eff, async |error| {
                tracing::error!(error = %error, tip = %msg, "failed to adopt tip");
            })
            .await;
        match adopt_result {
            AdoptTipResult::BestChainRolledForward | AdoptTipResult::BestChainSwitched => {}
            AdoptTipResult::HeaderNotFound => {
                tracing::error!(tip = %msg, "invariant violated: incoming header was loaded but find_ancestor_on_best_chain reports it missing");
                return eff.terminate().await;
            }
            AdoptTipResult::AncestorOnBestChainNotFound => {
                tracing::error!(tip = %msg, "invariant violated: incoming chain shares no ancestor with the best chain");
                return eff.terminate().await;
            }
        }
    } else {
        store
            .roll_forward_chain(&incoming_header.point())
            .or_terminate_with(&eff, async |error| {
                tracing::error!(error = %error, tip = %msg, "failed to adopt first tip");
            })
            .await;
    }

    drag_anchor_forward(&store, &msg, state.consensus_security_param)
        .or_terminate_with(&eff, async |error| {
            tracing::error!(error = %error, tip = %msg, "failed to drag anchor forward");
        })
        .await;

    // do not print every single block while catching up
    let now = eff.clock().await;
    if now.saturating_since(state.last_printed) >= Duration::from_secs(1) {
        tracing::info!(tip.slot = %msg.slot(), tip.hash = %msg.hash(), tip.block_height = %msg.block_height(), max_block_height = %state.max_block_height, suppressed = %state.suppressed, "adopted tip");
        state.last_printed = now;
        state.suppressed = 0;
    } else {
        tracing::debug!(tip.slot = %msg.slot(), tip.hash = %msg.hash(), tip.block_height = %msg.block_height(), max_block_height = %state.max_block_height, suppressed = %state.suppressed, "adopted tip");
        state.suppressed += 1;
    }
    eff.send(&state.mempool, MempoolMsg::NewTip(msg)).await;
    eff.send(&state.downstream, ManagerMessage::NewTip(msg)).await;
    eff.send(&state.block_source, BlockSourceMsg::AdoptedTip(msg)).await;
    state.current_best_tip = msg;
    state
}

/// Adopt the tip: update the best chain fragment and best chain hash in a single store transaction.
async fn adopt_tip(
    store: &Store,
    incoming_header: &BlockHeader,
    current_best: &BlockHeader,
) -> Result<AdoptTipResult, StoreError> {
    if incoming_header.parent() == Some(current_best.hash()) {
        store.roll_forward_chain(&incoming_header.point()).await?;
        return Ok(AdoptTipResult::BestChainRolledForward);
    }

    match store.find_ancestor_on_best_chain(incoming_header.hash()).await? {
        FindAncestorOnBestChainResult::StartHeaderNotFound => Ok(AdoptTipResult::HeaderNotFound),
        FindAncestorOnBestChainResult::NotFound => Ok(AdoptTipResult::AncestorOnBestChainNotFound),
        FindAncestorOnBestChainResult::Found { fork_point, forward_points } => {
            store.switch_to_fork(&fork_point, &forward_points).await?;
            Ok(AdoptTipResult::BestChainSwitched)
        }
    }
}

enum AdoptTipResult {
    HeaderNotFound,
    AncestorOnBestChainNotFound,
    BestChainRolledForward,
    BestChainSwitched,
}

/// Drag the store anchor forward so it is at most `consensus_security_param`
/// blocks behind the adopted tip. The walk along the best chain runs in a single
/// store effect against a consistent snapshot; only the resulting anchor write
/// is a separate effect. Only moves the anchor forward, never backward.
async fn drag_anchor_forward(store: &Store, tip: &Tip, consensus_security_param: u64) -> Result<(), StoreError> {
    let target_height = tip.block_height() - consensus_security_param;
    if let Some(new_anchor) = store.find_anchor_at_height(target_height).await {
        store.set_anchor_hash(&new_anchor).await?;
    }
    Ok(())
}

#[cfg(test)]
mod test_setup;
#[cfg(test)]
mod tests;
