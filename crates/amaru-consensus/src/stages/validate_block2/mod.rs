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

use amaru_kernel::{IsHeader, Peer, Point, Tip};
use amaru_metrics::ledger::LedgerMetrics;
use amaru_ouroboros::{BlockValidationError, ReadOnlyChainStore};
use amaru_protocols::store_effects::Store;
use pure_stage::{Effects, StageRef, TryInStage};

use crate::{
    effects::{Ledger, LedgerOps, Metrics, MetricsOps},
    errors::{ConsensusError, ValidationFailed},
    stages::select_chain_new::SelectChainMsg,
};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ValidateBlock {
    manager: StageRef<Tip>,
    selet_chain: StageRef<SelectChainMsg>,
    /// This is always at the tip of the ledger
    current: Point,
}

impl ValidateBlock {
    pub fn new(manager: StageRef<Tip>, selet_chain: StageRef<SelectChainMsg>, current: Point) -> Self {
        Self { manager, selet_chain, current }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ValidateBlockMsg {
    tip: Tip,
    parent: Point,
}

impl ValidateBlockMsg {
    pub fn new(tip: Tip, parent: Point) -> Self {
        Self { tip, parent }
    }
}

pub async fn stage(mut state: ValidateBlock, msg: ValidateBlockMsg, eff: Effects<ValidateBlockMsg>) -> ValidateBlock {
    if msg.parent == Point::Origin {
        tracing::info!(parent = %msg.parent, current = %state.current, tip = %msg.tip.point(), "skipping validation of genesis block");
        eff.send(&state.selet_chain, SelectChainMsg::BlockValidationResult(msg.tip, true)).await;
        state.current = msg.tip.point();
        return state;
    }

    let ledger = Ledger::new(eff.clone());
    let store = Store::new(eff.clone());
    tracing::debug!(parent = %msg.parent, current = %state.current, tip = %msg.tip.point(), "validating block");

    if msg.parent != state.current {
        // step 1: roll back to some known point
        // (this could be further back than the parent when switching forks)
        tracing::info!(parent = %msg.parent, current = %state.current, "rolling back ledger to common ancestor point");
        match roll_back_to_ancestor(&ledger, &store, msg.parent).await {
            Ok((point, forward_points)) => {
                state.current = point;
                // step 2: roll forward to the parent point if needed
                // (none of the ancestors is already known to be invalid, as ensured above)
                tracing::info!(parent = %msg.parent, current = %state.current, points = %forward_points.len(), "rolling forward ledger to reach parent");
                for point in forward_points {
                    match validate(point, &ledger).await {
                        Ok(metrics) => {
                            Metrics::new(&eff).record(metrics.into()).await;
                            state.current = point;
                        }
                        Err(error) => {
                            tracing::warn!(error = %error, point = %point, "invalid block");
                            eff.send(&state.selet_chain, SelectChainMsg::BlockValidationResult(msg.tip, false)).await;
                            return state;
                        }
                    }
                }
            }
            Err(err) => {
                tracing::warn!(error = %err.error, parent = %msg.parent, "failed to rollback ledger to parent point");
                eff.send(&state.selet_chain, SelectChainMsg::BlockValidationResult(msg.tip, false)).await;
                return state;
            }
        }
    }

    match validate(msg.tip.point(), &ledger).await {
        Ok(metrics) => {
            Metrics::new(&eff).record(metrics.into()).await;
            eff.send(&state.selet_chain, SelectChainMsg::BlockValidationResult(msg.tip, true)).await;
            eff.send(&state.manager, msg.tip).await;
            state.current = msg.tip.point();
        }
        Err(error) => {
            tracing::warn!(error = %error, point = %msg.tip.point(), "invalid block");
            eff.send(&state.selet_chain, SelectChainMsg::BlockValidationResult(msg.tip, false)).await;
        }
    }

    state
}

async fn validate(point: Point, ledger: &Ledger<ValidateBlockMsg>) -> Result<LedgerMetrics, BlockValidationError> {
    let ctx = opentelemetry::Context::current();
    ledger
        .validate_block(&Peer::new("unknown"), &point, ctx)
        .await
        .or_terminate(ledger.eff(), async |error| {
            tracing::error!(error = %error, %point, "failed to validate block");
        })
        .await
}

async fn roll_back_to_ancestor(
    ledger: &Ledger<ValidateBlockMsg>,
    store: &Store<ValidateBlockMsg>,
    parent: Point,
) -> Result<(Point, Vec<Point>), ValidationFailed> {
    if ledger.contains_point(&parent) {
        ledger.rollback(&Peer::new("unknown"), &parent, opentelemetry::Context::current()).await?;
        return Ok((parent, Vec::new()));
    }

    let ledger_tip = ledger.tip();
    let mut rb_point = None;
    let mut forward_points = Vec::new();
    for (ancestor, valid) in store.ancestors_with_validity(parent.hash()) {
        if valid == Some(false) {
            return Err(ValidationFailed::new(
                &Peer::new("unknown"),
                ConsensusError::RollbackBlockFailed(
                    parent,
                    anyhow::anyhow!("rollback point depends on invalid block").into(),
                ),
            ));
        }
        if ancestor.point() < ledger_tip {
            return Err(ValidationFailed::new(
                &Peer::new("unknown"),
                ConsensusError::RollbackBlockFailed(
                    parent,
                    anyhow::anyhow!("cannot rollback into the immutable db").into(),
                ),
            ));
        }
        if ancestor.point() == ledger_tip || ledger.contains_point(&ancestor.point()) {
            rb_point = Some(ancestor.point());
            break;
        }
        forward_points.push(ancestor.point());
    }
    forward_points.reverse();

    if let Some(rb_point) = rb_point {
        ledger.rollback(&Peer::new("unknown"), &rb_point, opentelemetry::Context::current()).await?;
        Ok((rb_point, forward_points))
    } else {
        Err(ValidationFailed::new(
            // TODO: figure out which peer to blame
            &Peer::new("unknown"),
            ConsensusError::RollbackBlockFailed(
                parent,
                anyhow::anyhow!("rollback point not found in volatile db").into(),
            ),
        ))
    }
}

#[cfg(test)]
mod test_setup;
#[cfg(test)]
mod tests;
