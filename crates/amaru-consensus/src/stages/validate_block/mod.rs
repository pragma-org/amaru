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

use amaru_kernel::{BlockHeight, Peer, Point, Tip};
use amaru_pure_stage::{Effects, OrTerminateWith, StageRef};

use crate::{
    effects::{Ledger, LedgerOps, Metrics, MetricsOps},
    stages::{adopt_chain::AdoptChainMsg, block_source::BlockSourceMsg, select_chain::SelectChainMsg},
};

/// ValidateBlock stage: thin validation dispatcher + result router for the consensus pipeline.
///
/// The stage is instantiated via `ValidateBlock::new(manager, select_chain, block_source, current)`
/// (initializing `max_block_height` to 0) and driven by `ValidateBlockMsg::new(tip, parent, max_block_height)`.
///
/// On receipt:
/// - If `parent == Point::Origin`: log error and `eff.terminate()` (no downstream signals).
/// - `state.max_block_height = msg.max_block_height.max(state.max_block_height)`.
/// - If `msg.parent != state.current`: invoke `roll_back_to_ancestor` (which may emit `contains_volatile_point`,
///   `rollback`, `load_header_with_validity`, etc. effects). On `Err` from the helper: send
///   `SelectChainMsg::BlockValidationResult(msg.tip, false)` and `BlockSourceMsg::Validation { valid: false, point: msg.tip.point() }`
///   then return (no adopt). On success, set `current` and (if needed) roll forward over `forward_points`, calling
///   `validate(...)` on each; any failure during forward sends `...Result(msg.tip, false)` + `Validation { valid: false, point }` (the failing ancestor)
///   and returns early.
/// - Always (if still running): call `validate(msg.tip.point(), ...)` (emits `ValidateBlockEffect` via `Ledger`).
///   - Success: record `LedgerMetrics`, send `SelectChainMsg::BlockValidationResult(msg.tip, true)`,
///     `BlockSourceMsg::Validation { valid: true, point: msg.tip.point() }`, and
///     `AdoptChainMsg::new(msg.tip, msg.max_block_height)` to manager; update `state.current = msg.tip.point()`.
///   - `Err`: log warn "invalid block", send `...Result(msg.tip, false)` + `Validation { valid: false, ... }` (no adopt, no current update).
///
/// Validation is never direct; it is always via external effects (handled by `ResourceBlockValidation` etc.).
/// The stage tracks "current" (ledger tip invariant) and max height but only signals adopt on *final tip success*.
/// Partial ancestor work (successful rollbacks/forwards) updates local state + metrics but produces no select/block_source/manager messages.
/// Error signaling for `valid: false` is *not* uniform: some paths send the false messages and continue; others
/// (ledger failures inside `validate`/`roll_back_to_ancestor`, genesis, certain rollback ops) hit `or_terminate_with` or direct `terminate` and produce no `false` signals (or terminate the stage entirely).
///
/// See `validate` and `roll_back_to_ancestor` helpers for details.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ValidateBlock {
    adopt_chain: StageRef<AdoptChainMsg>,
    select_chain: StageRef<SelectChainMsg>,
    block_source: StageRef<BlockSourceMsg>,
    /// This is always at the tip of the ledger
    current: Point,
    max_block_height: BlockHeight,
}

impl ValidateBlock {
    pub fn new(
        manager: StageRef<AdoptChainMsg>,
        select_chain: StageRef<SelectChainMsg>,
        block_source: StageRef<BlockSourceMsg>,
        current: Point,
    ) -> Self {
        Self { adopt_chain: manager, select_chain, block_source, current, max_block_height: BlockHeight::from(0) }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ValidateBlockMsg {
    tip: Tip,
    parent: Point,
    max_block_height: BlockHeight,
}

impl ValidateBlockMsg {
    pub fn new(tip: Tip, parent: Point, max_block_height: BlockHeight) -> Self {
        Self { tip, parent, max_block_height }
    }
}

pub async fn stage(mut state: ValidateBlock, msg: ValidateBlockMsg, eff: Effects<ValidateBlockMsg>) -> ValidateBlock {
    if msg.parent == Point::Origin {
        tracing::error!(parent = %msg.parent, current = %state.current, tip = %msg.tip.point(), "cannot start from genesis block");
        return eff.terminate().await;
    }

    state.max_block_height = msg.max_block_height.max(state.max_block_height);

    tracing::debug!(parent = %msg.parent, current = %state.current, tip = %msg.tip.point(), "validating block");
    let ctx = opentelemetry::Context::current();
    let ledger = Ledger::new(eff.clone());
    let peer = Peer::new("unknown");

    // simple chain extension: nothing to roll back, just apply the tip on top of current
    let result = if msg.parent == state.current {
        ledger
            .validate_block(&peer, &msg.tip.point(), ctx)
            .or_terminate_with(&eff, async |error| {
                tracing::error!(error = %error, tip = %msg.tip.point(), "failed to roll forward");
            })
            .await
    } else {
        tracing::info!(parent = %msg.parent, current = %state.current, "rolling back ledger to common ancestor point");
        ledger
            .switch_to_fork(&peer, &state.current, &msg.tip.point(), ctx)
            .or_terminate_with(&eff, async |error| {
                tracing::error!(error = %error, old_tip = %state.current, new_tip = %msg.tip.point(), parent = %msg.parent, "failed to switch to fork");
            })
            .await
    };

    match result {
        Ok(metrics) => {
            Metrics::new(&eff).record(metrics.into()).await;
            eff.send(&state.select_chain, SelectChainMsg::BlockValidationResult(msg.tip, true)).await;
            eff.send(&state.block_source, BlockSourceMsg::Validation { valid: true, point: msg.tip.point() }).await;
            eff.send(&state.adopt_chain, AdoptChainMsg::new(msg.tip, state.max_block_height)).await;
            state.current = msg.tip.point();
        }
        Err(error) => {
            tracing::warn!(%error, tip = %msg.tip, "failed to validate a block");
            eff.send(&state.select_chain, SelectChainMsg::BlockValidationResult(msg.tip, false)).await;
            eff.send(&state.block_source, BlockSourceMsg::Validation { valid: false, point: msg.tip.point() }).await;
        }
    }
    state
}

#[cfg(test)]
mod test_setup;
#[cfg(test)]
mod tests;
