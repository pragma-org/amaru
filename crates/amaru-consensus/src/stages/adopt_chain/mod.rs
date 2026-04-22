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

use std::{cmp::Ordering, time::Duration};

use amaru_kernel::{BlockHeader, BlockHeight, IsHeader, Tip};
use amaru_ouroboros_traits::{ChainStore, ReadOnlyChainStore, StoreError};
use amaru_protocols::{manager::ManagerMessage, store_effects::Store};
use pure_stage::{Effects, Instant, StageRef, TryInStage};

use crate::stages::select_chain::cmp_tip;

/// This stage receives validated chains in the form of a Tip and decides
/// whether to adopt that tip as the new downstream tip.
///
/// It only adopts when the incoming tip is strictly better than the current
/// best chain (e.g. when switching forks, a valid chain may not be the best).
///
/// It manages the best_chain_hash and best chain link pointers in the store
/// using roll_forward_chain and rollback_chain. When switching forks, it
/// finds the rollback point as the youngest point on the current best chain
/// that is an ancestor of the newly adopted tip.
///
/// The anchor is dragged at most `consensus_security_param` blocks behind
/// the newly adopted tip by walking backwards along the best chain.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AdoptChain {
    downstream: StageRef<ManagerMessage>,
    consensus_security_param: u64,
    current_best_tip: Tip,
    max_block_height: BlockHeight,
    last_printed: Instant,
    suppressed: u32,
}

impl AdoptChain {
    pub fn new(downstream: StageRef<ManagerMessage>, consensus_security_param: u64, current_best_tip: Tip) -> Self {
        Self {
            downstream,
            consensus_security_param,
            current_best_tip,
            max_block_height: BlockHeight::from(0),
            last_printed: Instant::at_offset(Duration::from_secs(0)),
            suppressed: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AdoptChainMsg {
    tip: Tip,
    max_block_height: BlockHeight,
}

impl AdoptChainMsg {
    pub fn new(tip: Tip, max_block_height: BlockHeight) -> Self {
        Self { tip, max_block_height }
    }
}

pub async fn stage(mut state: AdoptChain, msg: AdoptChainMsg, eff: Effects<AdoptChainMsg>) -> AdoptChain {
    state.max_block_height = msg.max_block_height.max(state.max_block_height);
    let AdoptChainMsg { tip: msg, .. } = msg;

    if msg.block_height() < state.current_best_tip.block_height() {
        tracing::debug!(tip = %msg, current_best_tip = %state.current_best_tip, "incoming tip shorter than current best, skipping");
        return state;
    }

    let store = Store::new(eff);

    let incoming_header = store
        .load_header(&msg.hash())
        .or_terminate(store.eff(), async |_| {
            tracing::warn!(tip = %msg, "failed to load incoming tip");
        })
        .await;

    let current_best = store
        .load_header(&state.current_best_tip.hash())
        .or_terminate(store.eff(), async |_| {
            tracing::warn!("failed to load current best");
        })
        .await;

    if cmp_tip(Some(&incoming_header), Some(&current_best)) != Ordering::Greater {
        tracing::debug!(tip = %msg, "incoming tip not better than current best, not adopting");
        return state;
    }

    adopt_tip(&store, &incoming_header, &current_best)
        .or_terminate(store.eff(), async |error| {
            tracing::error!(error = %error, tip = %msg, "failed to adopt tip");
        })
        .await;

    drag_anchor_forward(&store, &msg, state.consensus_security_param)
        .or_terminate(store.eff(), async |error| {
            tracing::error!(error = %error, tip = %msg, "failed to drag anchor forward");
        })
        .await;

    // do not print every single block while catching up
    let now = Instant::now();
    if now.saturating_since(state.last_printed) >= Duration::from_secs(1) {
        tracing::info!(tip.slot = %msg.slot(), tip.hash = %msg.hash(), tip.block_height = %msg.block_height(), max_block_height = %state.max_block_height, suppressed = %state.suppressed, "adopted tip");
        state.last_printed = now;
        state.suppressed = 0;
    } else {
        tracing::debug!(tip.slot = %msg.slot(), tip.hash = %msg.hash(), tip.block_height = %msg.block_height(), max_block_height = %state.max_block_height, suppressed = %state.suppressed, "adopted tip");
        state.suppressed += 1;
    }
    store.eff().send(&state.downstream, ManagerMessage::NewTip(msg)).await;
    state.current_best_tip = msg;
    state
}

/// Adopt the tip: update best_chain_hash and maintain best chain links via
/// roll_forward_chain and rollback_chain.
fn adopt_tip(
    store: &Store<AdoptChainMsg>,
    incoming_header: &BlockHeader,
    current_best: &BlockHeader,
) -> Result<(), StoreError> {
    if incoming_header.parent() == Some(current_best.hash()) {
        store.roll_forward_chain(&incoming_header.point())?;
    } else {
        mark_new_chain(store, incoming_header)?;
    }

    store.set_best_chain_hash(&incoming_header.hash())?;
    Ok(())
}

/// Find the youngest point on the current best chain that is an ancestor of
/// the given header (the intersection point when switching forks).
fn mark_new_chain(store: &Store<AdoptChainMsg>, incoming_header: &BlockHeader) -> Result<(), StoreError> {
    let mut forward_points = Vec::new();
    for ancestor in store.ancestors(incoming_header.clone()) {
        let point = ancestor.point();
        if store.load_from_best_chain(&point).is_some() {
            store.rollback_chain(&point)?;
            forward_points.reverse();
            for point in forward_points {
                store.roll_forward_chain(&point)?;
            }
            return Ok(());
        }
        forward_points.push(point);
    }
    Err(StoreError::ReadError { error: "rollback point not found: new tip has no ancestor on best chain".to_string() })
}

/// Drag the store anchor forward so it is at most `consensus_security_param`
/// blocks behind the adopted tip. Walks forward from the anchor along the
/// best chain using next_best_chain. Only moves the anchor forward, never backward.
fn drag_anchor_forward(
    store: &Store<AdoptChainMsg>,
    tip: &Tip,
    consensus_security_param: u64,
) -> Result<(), StoreError> {
    let target_anchor_height = tip.block_height() - consensus_security_param;

    if target_anchor_height.as_u64() == 0 {
        return Ok(());
    }

    let current_anchor_tip =
        store.load_header(&store.get_anchor_hash()).map(|h: BlockHeader| h.tip()).unwrap_or(Tip::origin());

    if target_anchor_height <= current_anchor_tip.block_height() {
        return Ok(());
    }

    let mut point = current_anchor_tip.point();

    while let Some(next_point) = store.next_best_chain(&point) {
        let next_header = match store.load_header(&next_point.hash()) {
            Some(h) => h,
            None => break,
        };
        if next_header.block_height() >= target_anchor_height {
            store.set_anchor_hash(&next_point.hash())?;
            return Ok(());
        }
        point = next_point;
    }

    Ok(())
}

#[cfg(test)]
mod test_setup;
#[cfg(test)]
mod tests;
