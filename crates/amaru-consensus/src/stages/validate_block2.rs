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

use amaru_kernel::{Peer, Point, Tip, cardano::network_block::NetworkBlock, cbor};
use amaru_ouroboros::{ReadOnlyChainStore, StoreError};
use amaru_protocols::{manager::ManagerMessage, store_effects::Store};
use pure_stage::{Effects, StageRef, TryInStage};

use crate::{
    effects::{Ledger, LedgerOps, Metrics, MetricsOps},
    stages::select_chain_new::SelectChainMsg,
};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ValidateBlock {
    manager: StageRef<ManagerMessage>,
    selet_chain: StageRef<SelectChainMsg>,
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
    let ledger = Ledger::new(eff);
    tracing::debug!(parent = %msg.parent, current = %state.current, tip = %msg.tip.point(), "validating block");
    if msg.parent != state.current {
        tracing::info!(parent = %msg.parent, current = %state.current, "rolling back ledger to parent point");
        ledger
            .rollback(&Peer::new("unknown"), &msg.parent, ctx.clone())
            .await
            .or_terminate(ledger.eff(), async |err| {
                tracing::error!(error = %err.error, parent = %msg.parent, "failed to rollback ledger to parent point");
            })
            .await;
        state.current = msg.parent;
    }
    let store = Store::new(ledger.eff().clone());
    let block = store
        .load_block(&msg.tip.hash())
        .and_then(|block| block.ok_or(StoreError::ReadError { error: "block not found".to_string() }))
        .or_terminate(store.eff(), async |error| {
            tracing::error!(error = %error, point = %msg.tip.point(), "failed to load block");
        })
        .await;
    let block = cbor::decode::<NetworkBlock>(&block)
        .or_terminate(store.eff(), async |error| {
            tracing::error!(error = %error, point = %msg.tip.point(), "failed to decode network block");
        })
        .await;
    let block = block
        .decode_block()
        .or_terminate(store.eff(), async |error| {
            tracing::error!(error = %error, point = %msg.tip.point(), "failed to decode block");
        })
        .await;
    let tip = block.tip();
    let result = ledger
        .validate_block(&Peer::new("unknown"), &msg.tip.point(), block, ctx)
        .await
        .or_terminate(ledger.eff(), async |error| {
            tracing::error!(error = %error, point = %msg.tip.point(), "failed to validate block");
        })
        .await;
    let valid = result.is_ok();
    match result {
        Ok(metrics) => {
            tracing::info!(point = %msg.tip.point(), "valid block");
            Metrics::new(ledger.eff()).record(metrics.into()).await;
            state.current = msg.tip.point();
        }
        Err(error) => {
            tracing::warn!(error = %error, point = %msg.tip.point(), "invalid block");
        }
    }
    ledger.eff().send(&state.selet_chain, SelectChainMsg::BlockValidationResult(msg.tip, valid)).await;
    ledger.eff().send(&state.manager, ManagerMessage::NewTip(tip)).await;

    state
}
