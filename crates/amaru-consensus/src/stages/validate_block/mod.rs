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

use std::collections::BTreeMap;

use amaru_kernel::{BlockHeight, HeaderHash, Point};
use amaru_metrics::LedgerMetrics;
use amaru_observability::{Instrument, TraceContext, debug, debug_record, debug_span, error, info, warn};
use amaru_ouroboros_traits::ForkSwitchOutcome;
use amaru_protocols::store_effects::Store;
use amaru_pure_stage::{Effects, OrTerminateWith, StageRef};

use crate::{
    effects::{Ledger, LedgerOps, Metrics, MetricsOps},
    stages::{
        adopt_chain::AdoptChainMsg,
        block_source::BlockSourceMsg,
        select_chain::{SelectChainMsg, cmp_tip},
    },
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
///     `BlockSourceMsg::Validation { valid: true, point: msg.tip }`, and
///     `AdoptChainMsg::new(msg.tip, state.max_block_height)` to manager; update `state.current = msg.tip`.
///   - `Err`: log warn, send `...Result(msg.tip, false)` + `Validation { valid: false, ... }` (no adopt, no current update).
/// - If `msg.tip` is not better than the current tip (using the [`cmp_tip`] function) the message is
///   dropped.
/// - Otherwise: ask the ledger to switch to the fork ending at `msg.tip` (`Ledger::switch_to_fork`,
///   a `SwitchToForkEffect`) and route its `ForkSwitchOutcome`:
///   - `Completed`: same signals as a successful extension.
///   - `Partial { applied_tip, failure, .. }`: the ledger kept the fork's valid prefix; signal `applied_tip`
///     as a successful extension (metrics, results, adopt, `current = applied_tip`), then send
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
    consensus_security_param: u64,
    /// Blocks that failed validation (with their heights), plus in-flight descendants refused
    /// because of them. Consulted to avoid asking the ledger to validate chains already known to
    /// be dead. Entries deeper than `consensus_security_param` below the ledger tip are evicted,
    /// since no chain forking that far down can ever be adopted.
    invalid_blocks: BTreeMap<HeaderHash, BlockHeight>,
}

impl ValidateBlock {
    pub fn new(
        manager: StageRef<AdoptChainMsg>,
        select_chain: StageRef<SelectChainMsg>,
        block_source: StageRef<BlockSourceMsg>,
        consensus_security_param: u64,
        current: Point,
    ) -> Self {
        Self {
            adopt_chain: manager,
            select_chain,
            block_source,
            consensus_security_param,
            current,
            max_block_height: BlockHeight::from(0),
            invalid_blocks: BTreeMap::new(),
        }
    }

    // Notify other stages of a successful block validation, record metrics, and update the current tip.
    pub async fn completed(
        &mut self,
        tip: Point,
        eff: &Effects<ValidateBlockMsg>,
        metrics: LedgerMetrics,
        trace_context: &TraceContext,
    ) {
        Metrics::new(eff).record(metrics.into()).await;
        eff.send(
            &self.select_chain,
            SelectChainMsg::block_validation_result(tip, true, self.max_block_height).with_trace_context(trace_context),
        )
        .await;
        eff.send(&self.block_source, BlockSourceMsg::Validation { valid: true, point: tip }).await;
        eff.send(&self.adopt_chain, AdoptChainMsg::new(tip, self.max_block_height).with_trace_context(trace_context))
            .await;

        // Condemned blocks deeper than k below the new tip can never have their header selected again
        let k = self.consensus_security_param;
        self.invalid_blocks.retain(|_, height| height.as_u64() + k > tip.block_height().as_u64());
        self.current = tip;
    }

    // Notify other stages of a failed block validation, record metrics, and update the current tip.
    pub async fn error(
        &mut self,
        msg: ValidateBlockMsg,
        failed_tip: Point,
        eff: &Effects<ValidateBlockMsg>,
        reason: &str,
        message: &str,
        trace_context: &TraceContext,
    ) {
        warn!(
            consensus::block::INVALID,
            failed_tip = failed_tip,
            parent = msg.parent,
            error = reason,
            detail = message
        );
        self.invalid_blocks.insert(failed_tip.hash(), failed_tip.block_height());
        self.invalid_blocks.insert(msg.tip.hash(), msg.tip.block_height());

        eff.send(
            &self.select_chain,
            SelectChainMsg::block_validation_result(failed_tip, false, self.max_block_height)
                .with_trace_context(trace_context),
        )
        .await;
        eff.send(&self.block_source, BlockSourceMsg::Validation { valid: false, point: failed_tip }).await;
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ValidateBlockMsg {
    tip: Point,
    parent: Point,
    max_block_height: BlockHeight,
    trace_context: TraceContext,
}

impl ValidateBlockMsg {
    pub fn new(tip: Point, parent: Point, max_block_height: BlockHeight) -> Self {
        Self { tip, parent, max_block_height, trace_context: Default::default() }
    }

    pub fn with_trace_context(mut self, trace_context: &TraceContext) -> Self {
        self.trace_context = trace_context.clone();
        self
    }
}

pub async fn stage(
    mut state: ValidateBlock,
    mut msg: ValidateBlockMsg,
    eff: Effects<ValidateBlockMsg>,
) -> ValidateBlock {
    let tip = msg.tip;
    if msg.parent == Point::Origin {
        error!(consensus::block::VALIDATE_FROM_GENESIS, tip = tip, current = state.current, parent = msg.parent);
        return eff.terminate().await;
    }

    let trace_context = std::mem::take(&mut msg.trace_context);
    let root_trace_context = trace_context.clone();
    let span = debug_span!(
            parent_context: trace_context,
            consensus::block::VALIDATE,
            tip = tip,
            header_hash = tip.hash());
    let stage_context = (&span).into();

    state.max_block_height = msg.max_block_height.max(state.max_block_height);
    async {
        let ledger = Ledger::new(eff.clone()).with_trace_context(&stage_context);
        debug_record!(consensus::block::VALIDATE, current = state.current, parent = msg.parent);

        // No need to validate a block that descends from an invalid block or has already been determined to
        // be invalid.
        if state.invalid_blocks.contains_key(&msg.parent.hash()) || state.invalid_blocks.contains_key(&tip.hash()) {
            state
                .error(
                    msg,
                    tip,
                    &eff,
                    "the block descends from an invalid block",
                    "refusing to validate the descendant of an invalid block",
                    &root_trace_context,
                )
                .await;
            return state;
        }

        if msg.parent == state.current {
            let result = ledger
                .validate_block(&tip)
                .or_terminate_with(&eff, async |err| {
                    warn!(consensus::block::APPLY_FAILED, tip = msg.tip, step = "validate_block", error = err.to_string());
                })
                .await;
            match result {
                Ok(metrics) => state.completed(tip, &eff, metrics, &root_trace_context).await,
                Err(err) => {
                    state
                        .error(
                            msg,
                            tip,
                            &eff,
                            &err.to_string(),
                            "failed to advance the ledger to a new tip",
                            &root_trace_context,
                        )
                        .await;
                }
            }
        } else {
            // fetch_blocks streams the blocks of a new best candidate one by one, each with its own
            // tip. Only a tip that is strictly better than the current one (per the chain-selection
            // order) can be accepted as a candidate for a fork switch on the ledger.
            //
            // NOTE: the headers are loaded from the store on demand rather than kept in the stage
            // state. This branch is neither on the sync hot path nor on the caught-up common path
            // (both extend `current` and take the branch above), while a header held in the state
            // would add ~1.5kB to every stage state snapshot serialized into the TraceBuffer.
            let store = Store::new(eff.clone());
            let message_header = store.load_header(&tip.hash()).await;
            let current_header = store.load_header(&state.current.hash()).await;
            if cmp_tip(message_header.as_ref(), current_header.as_ref()) != std::cmp::Ordering::Greater {
                debug!(consensus::block::SKIP, current = state.current, tip = tip);
                return state;
            }

            info!(consensus::block::SWITCH_FORK, current = state.current, parent = msg.parent);
            let result = ledger
                .switch_to_fork(&tip)
                .or_terminate_with(&eff, async |err| {
                    warn!(consensus::block::APPLY_FAILED, tip = msg.tip, step = "switch_to_fork", error = err.to_string());
                })
                .await;
            match result {
                ForkSwitchOutcome::Completed { metrics } => {
                    state.completed(tip, &eff, metrics, &root_trace_context).await
                }
                ForkSwitchOutcome::Partial { metrics, applied_tip, failure } => {
                    // mark blocks up to applied_tip as valid
                    state.completed(applied_tip, &eff, metrics, &root_trace_context).await;
                    // marks blocks from failure.tip as invalid and sends the appropriate signals to other stages
                    state
                        .error(
                            msg,
                            failure.tip,
                            &eff,
                            &failure.reason,
                            "fork switch partially applied",
                            &root_trace_context,
                        )
                        .await;
                }
                ForkSwitchOutcome::Failed { failure } => {
                    state
                        .error(
                            msg,
                            failure.tip,
                            &eff,
                            &failure.reason,
                            "failed to fork the ledger to a new tip",
                            &root_trace_context,
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

#[cfg(test)]
mod test_setup;
#[cfg(test)]
mod tests;
