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

use amaru_kernel::{BlockHeight, Point, Tip};
use amaru_metrics::LedgerMetrics;
use amaru_observability::{TraceContext, debug_span};
use amaru_ouroboros_traits::ForkSwitchOutcome;
use amaru_pure_stage::{Effects, OrTerminateWith, StageRef};
use tracing::Instrument;

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
/// - If `msg.parent == state.current`: the block extends the ledger and is validated via
///   `Ledger::validate_block` (a `ValidateBlockEffect`).
///   - Success: record `LedgerMetrics`, send `SelectChainMsg::BlockValidationResult(msg.tip, true, state.max_block_height)`,
///     `BlockSourceMsg::Validation { valid: true, point: msg.tip.point() }`, and
///     `AdoptChainMsg::new(msg.tip, state.max_block_height)` to manager; update `state.current = msg.tip.point()`.
///   - `Err`: log warn, send `...Result(msg.tip, false)` + `Validation { valid: false, ... }` (no adopt, no current update).
/// - Otherwise: ask the ledger to switch to the fork ending at `msg.tip` (`Ledger::switch_to_fork`,
///   a `SwitchToForkEffect`) and route its `ForkSwitchOutcome`:
///   - `Completed`: same signals as a successful extension.
///   - `Partial { applied_tip, failure, .. }`: the ledger kept the fork's valid prefix; signal `applied_tip`
///     as a successful extension (metrics, results, adopt, `current = applied_tip.point()`), then send
///     `...Result(failure.tip, false)` + `Validation { valid: false, ... }` for the failing block. No result
///     is sent for `msg.tip` itself: select_chain condemns descendants of an invalid block transitively.
///   - `RolledBack { failure }`: the ledger restored its pre-switch state; send
///     `...Result(failure.tip, false)` + `Validation { valid: false, ... }` (the failing block may differ
///     from `msg.tip`).
///
/// Validation is never direct; it is always via external effects (handled by `ResourceBlockValidation` etc.).
/// Ledger infrastructure errors hit `or_terminate_with` and terminate the stage without `false` signals.
///
/// See the `completed` and `error` helpers for the exact signal sequences.
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
    let tip = msg.tip;
    if msg.parent == Point::Origin {
        tracing::error!(parent = %msg.parent, current = %state.current, tip = %tip, "cannot start from genesis block");
        return eff.terminate().await;
    }

    let span = debug_span!(
            parent_context: msg.trace_context,
            consensus::block::VALIDATE,
            tip = tip,
            header_hash = tip.hash());
    let stage_context = (&span).into();

    state.max_block_height = msg.max_block_height.max(state.max_block_height);
    async {
        let ledger = Ledger::new(eff.clone()).with_trace_context(&stage_context);
        tracing::debug!(parent = %msg.parent, current = %state.current, tip = %tip, "validating block");

        if msg.parent == state.current {
            let result = ledger
                .validate_block(&tip.point())
                .or_terminate_with(&eff, async |err| {
                    tracing::warn!(tip = %msg.tip, err = %err, "failed to validate the new block");
                })
                .await;
            match result {
                Ok(metrics) => completed(&mut state, tip, &eff, metrics).await,
                Err(err) => {
                    error(
                        &mut state,
                        tip,
                        msg.parent,
                        &eff,
                        &err.to_string(),
                        "failed to advance the ledger to a new tip",
                    )
                    .await;
                }
            }
        } else {
            tracing::info!(parent = %msg.parent, current = %state.current, "switching the ledger to a new fork");
            let result = ledger
                .switch_to_fork(&tip)
                .or_terminate_with(&eff, async |err| {
                    tracing::warn!(tip = %msg.tip, err = %err, "failed to switch to a new fork");
                })
                .await;
            match result {
                ForkSwitchOutcome::Completed { metrics } => completed(&mut state, tip, &eff, metrics).await,
                ForkSwitchOutcome::Partial { metrics, applied_tip, failure } => {
                    // mark blocks up to applied_tip as valid
                    completed(&mut state, applied_tip, &eff, metrics).await;
                    // marks blocks from failure.tip as invalid and sends the appropriate signals to other stages
                    error(&mut state, failure.tip, msg.parent, &eff, &failure.reason, "fork switch partially applied")
                        .await;
                }
                ForkSwitchOutcome::Failed { failure } => {
                    error(
                        &mut state,
                        failure.tip,
                        msg.parent,
                        &eff,
                        &failure.reason,
                        "failed to fork the ledger to a new tip",
                    )
                    .await;
                }
            }
        };
        state
    }
    .instrument(span)
    .await
}

async fn completed(state: &mut ValidateBlock, tip: Tip, eff: &Effects<ValidateBlockMsg>, metrics: LedgerMetrics) {
    Metrics::new(eff).record(metrics.into()).await;
    eff.send(&state.select_chain, SelectChainMsg::BlockValidationResult(tip, true, state.max_block_height)).await;
    eff.send(&state.block_source, BlockSourceMsg::Validation { valid: true, point: tip.point() }).await;
    eff.send(&state.adopt_chain, AdoptChainMsg::new(tip, state.max_block_height)).await;
    state.current = tip.point();
}

async fn error(
    state: &mut ValidateBlock,
    tip: Tip,
    parent: Point,
    eff: &Effects<ValidateBlockMsg>,
    reason: &str,
    message: &str,
) {
    tracing::warn!(error = %reason, parent = %parent, message);
    eff.send(&state.select_chain, SelectChainMsg::BlockValidationResult(tip, false, state.max_block_height)).await;
    eff.send(&state.block_source, BlockSourceMsg::Validation { valid: false, point: tip.point() }).await;
}

#[cfg(test)]
mod test_setup;
#[cfg(test)]
mod tests;
