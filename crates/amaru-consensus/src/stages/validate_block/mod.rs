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
use amaru_ouroboros::BlockValidationError;
use amaru_protocols::store_effects::Store;
use pure_stage::{Effects, OrTerminateWith, StageRef, TryInStage};

use crate::{
    effects::{Ledger, LedgerOps, Metrics, MetricsOps},
    errors::{ConsensusError, ValidationFailed},
    stages::{adopt_chain::AdoptChainMsg, block_source::BlockSourceMsg, select_chain::SelectChainMsg},
};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ValidateBlock {
    manager: StageRef<AdoptChainMsg>,
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
        Self { manager, select_chain, block_source, current, max_block_height: BlockHeight::from(0) }
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

    let ledger = Ledger::new(eff.clone());
    let store = Store::new(eff.clone());
    tracing::debug!(parent = %msg.parent, current = %state.current, tip = %msg.tip.point(), "validating block");

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
            tracing::debug!(point = %point, "validating block (roll forward)");
            match validate(point, &ledger, &eff).await {
                Ok(metrics) => {
                    Metrics::new(&eff).record(metrics.into()).await;
                    state.current = point;
                }
                Err(error) => {
                    tracing::warn!(error = %error, point = %point, "invalid block");
                    eff.send(&state.select_chain, SelectChainMsg::BlockValidationResult(msg.tip, false)).await;
                    eff.send(&state.block_source, BlockSourceMsg::Validation { valid: false, point }).await;
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
            eff.send(&state.manager, AdoptChainMsg::new(msg.tip, msg.max_block_height)).await;
            state.current = msg.tip.point();
        }
        Err(error) => {
            tracing::warn!(error = %error, point = %msg.tip.point(), "invalid block");
            eff.send(&state.select_chain, SelectChainMsg::BlockValidationResult(msg.tip, false)).await;
            eff.send(&state.block_source, BlockSourceMsg::Validation { valid: false, point: msg.tip.point() }).await;
        }
    }

    state
}

async fn validate(
    point: Point,
    ledger: &Ledger,
    eff: &Effects<ValidateBlockMsg>,
) -> Result<LedgerMetrics, BlockValidationError> {
    let ctx = opentelemetry::Context::current();
    ledger
        .validate_block(&Peer::new("unknown"), &point, ctx)
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
    if ledger.contains_point(&parent).await {
        ledger
            .rollback(&Peer::new("unknown"), &parent, opentelemetry::Context::current())
            .or_terminate_with(eff, async move |error| {
                tracing::error!(point = %parent, %error, "ledger volatile DB contains point by rollback failed");
            })
            .await;
        return Ok((parent, Vec::new()));
    }

    // search will abort at this point
    let ledger_tip = ledger.tip().await.point();
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

        if current_point <= ledger_tip {
            return Err(ConsensusError::InvalidRollback {
                peer,
                rollback_point: current_hash,
                max_point: ledger_tip.hash(),
            }
            .into());
        }

        if ledger.contains_point(&current_point).await {
            forward_points.reverse();
            ledger.rollback(&Peer::new(""), &current_point, opentelemetry::Context::current()).or_terminate_with(eff, async move |error| {
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
