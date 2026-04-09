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
use pure_stage::{Effects, OrTerminateWith, StageRef};

use crate::{
    effects::{Ledger, LedgerOps, Metrics, MetricsOps},
    errors::{ConsensusError, ValidationFailed},
    stages::{adopt_chain::AdoptChainMsg, select_chain::SelectChainMsg},
};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ValidateBlock {
    manager: StageRef<AdoptChainMsg>,
    selet_chain: StageRef<SelectChainMsg>,
    /// This is always at the tip of the ledger
    current: Point,
    max_block_height: BlockHeight,
}

impl ValidateBlock {
    pub fn new(manager: StageRef<AdoptChainMsg>, selet_chain: StageRef<SelectChainMsg>, current: Point) -> Self {
        Self { manager, selet_chain, current, max_block_height: BlockHeight::from(0) }
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
        match roll_back_to_ancestor(&ledger, &store, msg.parent).await {
            Ok((point, forward_points)) => {
                state.current = point;
                // step 2: roll forward to the parent point if needed
                // (none of the ancestors is already known to be invalid, as ensured above)
                tracing::info!(parent = %msg.parent, current = %state.current, points = %forward_points.len(), "rolling forward ledger to reach parent");
                for point in forward_points {
                    tracing::debug!(point = %point, "validating block (roll forward)");
                    match validate(point, &ledger, &eff).await {
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
                if matches!(&err.error, ConsensusError::LedgerContainsPointButRollbackFailed(..)) {
                    tracing::error!(
                        error = %err.error,
                        parent = %msg.parent,
                        "ledger inconsistency: contains_point was true but rollback failed"
                    );
                    return eff.terminate().await;
                }
                tracing::warn!(error = %err.error, parent = %msg.parent, "failed to rollback ledger to parent point");
                eff.send(&state.selet_chain, SelectChainMsg::BlockValidationResult(msg.tip, false)).await;
                return state;
            }
        }
    }

    match validate(msg.tip.point(), &ledger, &eff).await {
        Ok(metrics) => {
            Metrics::new(&eff).record(metrics.into()).await;
            eff.send(&state.selet_chain, SelectChainMsg::BlockValidationResult(msg.tip, true)).await;
            eff.send(&state.manager, AdoptChainMsg::new(msg.tip, msg.max_block_height)).await;
            state.current = msg.tip.point();
        }
        Err(error) => {
            tracing::warn!(error = %error, point = %msg.tip.point(), "invalid block");
            eff.send(&state.selet_chain, SelectChainMsg::BlockValidationResult(msg.tip, false)).await;
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

fn validation_failed_when_contains_claimed_rollback(rollback_target: Point, vf: ValidationFailed) -> ValidationFailed {
    #[allow(clippy::wildcard_enum_match_arm)]
    match vf.error {
        ConsensusError::RollbackBlockFailed(_, source) => ValidationFailed::new(
            &Peer::new("unknown"),
            ConsensusError::LedgerContainsPointButRollbackFailed(rollback_target, source),
        ),
        _ => vf,
    }
}

async fn roll_back_to_ancestor(
    ledger: &Ledger,
    store: &Store,
    parent: Point,
) -> Result<(Point, Vec<Point>), ValidationFailed> {
    if ledger.contains_point(&parent).await {
        return match ledger.rollback(&Peer::new("unknown"), &parent, opentelemetry::Context::current()).await {
            Ok(()) => Ok((parent, Vec::new())),
            Err(vf) => Err(validation_failed_when_contains_claimed_rollback(parent, vf)),
        };
    }

    match find_rollback_point(ledger, store, parent).await {
        RollbackPointSearchResult::Found { point, forward_points, chosen_because_contains } => {
            match ledger.rollback(&Peer::new("unknown"), &point, opentelemetry::Context::current()).await {
                Ok(()) => Ok((point, forward_points)),
                Err(vf) if chosen_because_contains => Err(validation_failed_when_contains_claimed_rollback(point, vf)),
                Err(vf) => Err(vf),
            }
        }
        RollbackPointSearchResult::DependsOnInvalid => Err(ValidationFailed::new(
            &Peer::new("unknown"),
            ConsensusError::RollbackBlockFailed(
                parent,
                anyhow::anyhow!("rollback point depends on invalid block").into(),
            ),
        )),
        RollbackPointSearchResult::BelowImmutable => Err(ValidationFailed::new(
            &Peer::new("unknown"),
            ConsensusError::RollbackBlockFailed(
                parent,
                anyhow::anyhow!("cannot rollback into the immutable db").into(),
            ),
        )),
        RollbackPointSearchResult::NotFound => Err(ValidationFailed::new(
            &Peer::new("unknown"),
            ConsensusError::RollbackBlockFailed(
                parent,
                anyhow::anyhow!("rollback point not found in volatile db").into(),
            ),
        )),
    }
}

async fn find_rollback_point(ledger: &Ledger, store: &Store, parent: Point) -> RollbackPointSearchResult {
    let ledger_tip = ledger.tip().await;
    let anchor_hash = store.get_anchor_hash().await;
    let mut current_hash = parent.hash();
    let mut forward_points = Vec::new();

    loop {
        let Some((ancestor, valid)) = store.load_header_with_validity(&current_hash).await else {
            return RollbackPointSearchResult::NotFound;
        };

        if valid == Some(false) {
            return RollbackPointSearchResult::DependsOnInvalid;
        }

        if ancestor.point() < ledger_tip.point() {
            return RollbackPointSearchResult::BelowImmutable;
        }

        let chosen_because_contains =
            ancestor.point() != ledger_tip.point() && ledger.contains_point(&ancestor.point()).await;
        if ancestor.point() == ledger_tip.point() || chosen_because_contains {
            forward_points.reverse();
            return RollbackPointSearchResult::Found {
                point: ancestor.point(),
                forward_points,
                chosen_because_contains,
            };
        }

        forward_points.push(ancestor.point());
        if current_hash == anchor_hash {
            return RollbackPointSearchResult::NotFound;
        }

        let Some(parent_hash) = ancestor.parent_hash() else {
            return RollbackPointSearchResult::NotFound;
        };
        current_hash = parent_hash;
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
enum RollbackPointSearchResult {
    Found { point: Point, forward_points: Vec<Point>, chosen_because_contains: bool },
    DependsOnInvalid,
    BelowImmutable,
    NotFound,
}

#[cfg(test)]
mod test_setup;
#[cfg(test)]
mod tests;
