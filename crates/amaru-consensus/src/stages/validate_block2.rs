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

use amaru_kernel::{Block, Peer, Point, Tip, cbor};
use amaru_ouroboros::{ReadOnlyChainStore, StoreError};
use amaru_protocols::store_effects::Store;
use pure_stage::{Effects, StageRef, TryInStage};

use crate::{
    effects::{Ledger, LedgerOps},
    stages::select_chain_new::SelectChainMsg,
};

pub struct ValidateBlock {
    manager: StageRef<Tip>,
    selet_chain: StageRef<SelectChainMsg>,
    current: Point,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ValidateBlockMsg {
    point: Point,
    parent: Point,
}

pub async fn stage(mut state: ValidateBlock, msg: ValidateBlockMsg, eff: Effects<ValidateBlockMsg>) -> ValidateBlock {
    let ctx = opentelemetry::Context::current();
    let ledger = Ledger::new(eff);
    if msg.parent != state.current {
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
        .load_block(&msg.point.hash())
        .and_then(|block| block.ok_or(StoreError::ReadError { error: "block not found".to_string() }))
        .or_terminate(store.eff(), async |error| {
            tracing::error!(error = %error, point = %msg.point, "failed to load block");
        })
        .await;
    let block = cbor::decode::<Block>(&block)
        .or_terminate(store.eff(), async |error| {
            tracing::error!(error = %error, point = %msg.point, "failed to decode block");
        })
        .await;
    let tip = block.tip();
    let result = ledger
        .validate_block(&Peer::new("unknown"), &msg.point, block, ctx)
        .await
        .or_terminate(ledger.eff(), async |error| {
            tracing::error!(error = %error, point = %msg.point, "failed to validate block");
        })
        .await;
    let valid = result.is_ok();
    match result {
        Ok(_metrics) => {
            tracing::info!(point = %msg.point, "valid block");
            state.current = msg.point;
        }
        Err(error) => {
            tracing::warn!(error = %error, point = %msg.point, "invalid block");
        }
    }
    ledger.eff().send(&state.selet_chain, SelectChainMsg::BlockValidationResult(msg.point, valid)).await;
    ledger.eff().send(&state.manager, tip).await;

    state
}
