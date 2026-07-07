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

use amaru_kernel::{BlockHeight, IsHeader, Peer, Point, Tip};
use amaru_metrics::ledger::LedgerMetrics;
use amaru_observability::{TraceContext, debug_record, debug_span};
use amaru_ouroboros::BlockValidationError;
use amaru_protocols::store_effects::Store;
use amaru_pure_stage::{Effects, OrTerminateWith, StageRef, TryInStage};
use tracing::Instrument;

use crate::{
    effects::{Ledger, LedgerOps, Metrics, MetricsOps},
    errors::{ConsensusError, ValidationFailed},
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
    trace_context: TraceContext,
}

impl ValidateBlockMsg {
    pub fn new(tip: Tip, parent: Point, max_block_height: BlockHeight) -> Self {
        Self { tip, parent, max_block_height, trace_context: Default::default() }
    }

    pub fn with_trace_context(mut self, trace_context: &TraceContext) -> Self {
        self.trace_context = trace_context.clone();
        self
    }
}

pub async fn stage(mut state: ValidateBlock, msg: ValidateBlockMsg, eff: Effects<ValidateBlockMsg>) -> ValidateBlock {
    if msg.parent == Point::Origin {
        tracing::error!(parent = %msg.parent, current = %state.current, tip = %msg.tip.point(), "cannot start from genesis block");
        return eff.terminate().await;
    }

    state.max_block_height = msg.max_block_height.max(state.max_block_height);

    let tip = msg.tip;
    let parent_context = msg.trace_context.clone();
    let span = debug_span!(
            parent_context: msg.trace_context,
            consensus::block::VALIDATE,
            tip = tip,
            header_hash = tip.hash());
    let stage_context = (&span).into();

    async {
        let ledger = Ledger::new(eff.clone()).with_trace_context(&stage_context);
        let store = Store::new(eff.clone()).with_trace_context(&stage_context);
        tracing::debug!(parent = %msg.parent, current = %state.current, tip = %msg.tip.point(), "validating block");

        // NOTE: rollback/roll-forward only ever pass blocks that have already been validated
        // Therefore, validation results always refer to `msg.tip`.

        if msg.parent != state.current {
            // step 1: roll back to some known point
            // (this could be further back than the parent when switching forks)
            tracing::info!(parent = %msg.parent, current = %state.current, "rolling back ledger to common ancestor point");
            let (point, forward_points) = match roll_back_to_ancestor(&ledger, &store, &eff, msg.parent).await {
                Ok(x) => x,
                Err(err) => {
                    // NOTE: we only get peer errors here, all local failures are already handled
                    tracing::warn!(error = %err.error, parent = %msg.parent, "failed to rollback ledger to parent point");
                    eff.send(&state.select_chain, SelectChainMsg::BlockValidationResult(msg.tip, false)).await;
                    eff.send(&state.block_source, BlockSourceMsg::Validation { valid: false, point: msg.tip.point() })
                        .await;
                    return state;
                }
            };
            state.current = point;
            // step 2: roll forward to the parent point if needed
            // (none of the ancestors is already known to be invalid, as ensured above)
            tracing::info!(parent = %msg.parent, current = %state.current, points = %forward_points.len(), "rolling forward ledger to reach parent");
            let to_do = forward_points.len();
            let mut done = 0;
            for point in forward_points {
                tracing::debug!(%point, "validating block (roll forward)");
                match validate(point, &ledger, &eff).await {
                    Ok(metrics) => {
                        Metrics::new(&eff).record(metrics.into()).await;
                        state.current = point;
                    }
                    Err(error) => {
                        tracing::error!(%error, %point, "invalid block while spooling forward (this may be okay right after node restart)");
                        eff.send(&state.select_chain, SelectChainMsg::BlockValidationResult(msg.tip, false)).await;
                        eff.send(&state.block_source, BlockSourceMsg::Validation { valid: false, point: msg.tip.point() })
                            .await;
                        return state;
                    }
                }
                done += 1;
                if done % 100 == 0 {
                    tracing::info!(%done, %to_do, "rolling forward ledger to reach parent");
                }
            }
        }

        match validate(msg.tip.point(), &ledger, &eff).await {
            Ok(metrics) => {
                Metrics::new(&eff).record(metrics.into()).await;
                eff.send(&state.select_chain, SelectChainMsg::BlockValidationResult(msg.tip, true)).await;
                eff.send(&state.block_source, BlockSourceMsg::Validation { valid: true, point: msg.tip.point() }).await;
                eff.send(&state.adopt_chain, AdoptChainMsg::new(msg.tip, state.max_block_height).with_trace_context(&parent_context)).await;
                debug_record!(consensus::block::VALIDATE, valid = true);
                state.current = msg.tip.point();
            }
            Err(error) => {
                tracing::warn!(error = %error, point = %msg.tip.point(), "invalid block");
                debug_record!(consensus::block::VALIDATE, valid = false);
                eff.send(&state.select_chain, SelectChainMsg::BlockValidationResult(msg.tip, false)).await;
                eff.send(&state.block_source, BlockSourceMsg::Validation { valid: false, point: msg.tip.point() }).await;
            }
        }

        state
    }.instrument(span).await
}

async fn validate(
    point: Point,
    ledger: &Ledger,
    eff: &Effects<ValidateBlockMsg>,
) -> Result<LedgerMetrics, BlockValidationError> {
    ledger
        .validate_block(&Peer::new("unknown"), &point)
        .or_terminate_with(eff, async |error| {
            tracing::error!(error = %error, %point, "failed to validate block");
        })
        .await
}

async fn roll_back_to_ancestor(
    ledger: &Ledger,
    store: &Store,
    eff: &Effects<ValidateBlockMsg>,
    parent: Point,
) -> Result<(Point, Vec<Point>), ValidationFailed> {
    if ledger.contains_volatile_point(&parent).await {
        tracing::debug!(hash = %parent, "ledger contains parent point");
        ledger
            .rollback(&Peer::new("unknown"), &parent)
            .or_terminate_with(eff, async move |error| {
                tracing::error!(point = %parent, %error, "ledger volatile DB contains point by rollback failed");
            })
            .await;
        return Ok((parent, Vec::new()));
    }

    // search will abort at this point
    let ledger_tip = ledger.immutable_tip().await.point();
    let mut current_hash = parent.hash();
    let mut forward_points = Vec::new();
    // pseudo-peer because here we don't know which peer the block came from
    let peer = Peer::new("");

    loop {
        let (current_header, valid) = store
            .load_header_with_validity(&current_hash)
            .or_terminate_with(eff, async move |_| {
                tracing::error!(%current_hash, "failed to load header from store while searching for rollback point");
            })
            .await;
        let current_point = current_header.point();

        if valid == Some(false) {
            return Err(ConsensusError::BlockBuiltOnInvalidBlock { point: parent, invalid: current_point }.into());
        }

        if current_point < ledger_tip {
            return Err(ConsensusError::InvalidRollback {
                peer,
                rollback_point: current_hash,
                max_point: ledger_tip.hash(),
            }
            .into());
        }

        if current_point == ledger_tip || ledger.contains_volatile_point(&current_point).await {
            forward_points.reverse();
            ledger.rollback(&Peer::new(""), &current_point).or_terminate_with(eff, async move |error| {
                tracing::error!(point = %current_point, %error, "ledger volatile DB contains point but rollback failed");
            }).await;
            return Ok((current_point, forward_points));
        }

        forward_points.push(current_point);

        current_hash = current_header
            .parent()
            .or_terminate(eff, async move |_| {
                // NOTE: parent links are validated by track_peers already, and we are younger than ledger_tip
                tracing::error!(%current_hash, "reached genesis block while searching for rollback point");
            })
            .await;
    }
}

#[cfg(test)]
mod test_setup;
#[cfg(test)]
mod tests;
