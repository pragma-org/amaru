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

use amaru_kernel::{IsHeader, Peer, Point, Tip, cardano::network_block::NetworkBlock, cbor};
use amaru_ouroboros::{BlockValidationError, ReadOnlyChainStore, StoreError};
use amaru_protocols::{manager::ManagerMessage, store_effects::Store};
use pure_stage::{Effects, StageRef, TryInStage};

use crate::{
    effects::{Ledger, LedgerOps, Metrics, MetricsOps},
    errors::{ConsensusError, ValidationFailed},
    stages::select_chain_new::SelectChainMsg,
};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ValidateBlock {
    manager: StageRef<ManagerMessage>,
    selet_chain: StageRef<SelectChainMsg>,
    /// This is always at the tip of the ledger
    current: Point,
}

impl ValidateBlock {
    pub fn new(manager: StageRef<ManagerMessage>, selet_chain: StageRef<SelectChainMsg>, current: Point) -> Self {
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

    let ctx = opentelemetry::Context::current();
    let ledger = Ledger::new(eff.clone());
    let store = Store::new(eff);
    tracing::debug!(parent = %msg.parent, current = %state.current, tip = %msg.tip.point(), "validating block");

    if msg.parent != state.current {
        // step 1: roll back to some known point
        // (this could be further back than the parent when switching forks)
        tracing::info!(parent = %msg.parent, current = %state.current, "rolling back ledger to parent point");
        match roll_back_to_ancestor(&ledger, &store, msg.parent).await {
            Ok(point) => state.current = point,
            Err(err) => {
                tracing::error!(error = %err.error, parent = %msg.parent, "failed to rollback ledger to parent point");
                ledger.eff().send(&state.selet_chain, SelectChainMsg::BlockValidationResult(msg.tip, false)).await;
                return state;
            }
        }
    }
    if msg.parent != state.current {
        // step 2: roll forward to the parent point if needed
        // (none of the ancestors is already known to be invalid, as ensured above)
        let mut points = store
            .ancestors(
                store
                    .load_header(&msg.parent.hash())
                    .or_terminate(store.eff(), async |_| {
                        tracing::error!(point = %msg.parent, "failed to load header");
                    })
                    .await,
            )
            .map(|h| h.point())
            .take_while(|p| p != &state.current)
            .collect::<Vec<_>>();
        tracing::info!(parent = %msg.parent, current = %state.current, points = %points.len(), "rolling forward ledger to reach parent");
        points.reverse();
        for point in points {
            match validate(point, ctx.clone(), &ledger, &store).await {
                Ok((_tip, metrics)) => {
                    Metrics::new(ledger.eff()).record(metrics.into()).await;
                    state.current = point;
                }
                Err(error) => {
                    tracing::error!(error = %error, point = %point, "invalid block");
                    ledger.eff().send(&state.selet_chain, SelectChainMsg::BlockValidationResult(msg.tip, false)).await;
                    return state;
                }
            }
        }
    }

    match validate(msg.tip.point(), ctx, &ledger, &store).await {
        Ok((tip, metrics)) => {
            Metrics::new(ledger.eff()).record(metrics.into()).await;
            ledger.eff().send(&state.selet_chain, SelectChainMsg::BlockValidationResult(tip, true)).await;
            ledger.eff().send(&state.manager, ManagerMessage::NewTip(tip)).await;
            state.current = tip.point();
        }
        Err(error) => {
            tracing::error!(error = %error, point = %msg.tip.point(), "invalid block");
            ledger.eff().send(&state.selet_chain, SelectChainMsg::BlockValidationResult(msg.tip, false)).await;
        }
    }

    state
}

async fn validate(
    point: Point,
    ctx: opentelemetry::Context,
    ledger: &Ledger<ValidateBlockMsg>,
    store: &Store<ValidateBlockMsg>,
) -> Result<(Tip, amaru_metrics::ledger::LedgerMetrics), BlockValidationError> {
    let block = store
        .load_block(&point.hash())
        .and_then(|block| block.ok_or(StoreError::ReadError { error: "block not found".to_string() }))
        .or_terminate(store.eff(), async |error| {
            tracing::error!(error = %error, %point, "failed to load block");
        })
        .await;
    let block = cbor::decode::<NetworkBlock>(&block)
        .or_terminate(store.eff(), async |error| {
            tracing::error!(error = %error, %point, "failed to decode network block");
        })
        .await;
    let block = block
        .decode_block()
        .or_terminate(store.eff(), async |error| {
            tracing::error!(error = %error, %point, "failed to decode block");
        })
        .await;
    let tip = block.tip();
    let result = ledger
        .validate_block(&Peer::new("unknown"), &point, block, ctx)
        .await
        .or_terminate(ledger.eff(), async |error| {
            tracing::error!(error = %error, %point, "failed to validate block");
        })
        .await;
    result.map(|metrics| (tip, metrics))
}

async fn roll_back_to_ancestor(
    ledger: &Ledger<ValidateBlockMsg>,
    store: &Store<ValidateBlockMsg>,
    parent: Point,
) -> Result<Point, ValidationFailed> {
    if ledger.contains_point(&parent) {
        return Ok(parent);
    }

    let ledger_tip = ledger.tip();
    let mut rb_point = None;
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
        if ancestor.point() == ledger_tip || ledger.contains_point(&ancestor.point()) {
            rb_point = Some(ancestor.point());
            break;
        }
    }

    if let Some(rb_point) = rb_point {
        ledger.rollback(&Peer::new("unknown"), &rb_point, opentelemetry::Context::current()).await?;
        Ok(rb_point)
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
