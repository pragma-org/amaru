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

use std::{cmp::Ordering, collections::BTreeMap};

use amaru_kernel::{HeaderHash, IsHeader, Point, Tip};
use amaru_ouroboros::{ChainStore, ReadOnlyChainStore};
use amaru_protocols::store_effects::Store;
use pure_stage::{Effects, StageRef, TryInStage};

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SelectChain {
    downstream: StageRef<Tip>,
    /// Maps all block tree tips to the list of headers whose blocks are yet to be validated
    /// (oldest first)
    tips: BTreeMap<HeaderHash, Vec<HeaderHash>>,
    best_tip: Tip,
}

impl SelectChain {
    pub fn new(downstream: StageRef<Tip>, best_tip: Tip) -> Self {
        let mut tips = BTreeMap::new();
        if best_tip != Tip::origin() {
            tips.insert(best_tip.hash(), vec![]);
        }
        Self { downstream, best_tip, tips }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SelectChainMsg {
    TipFromUpstream(Tip),
    BlockValidationResult(Point, bool),
}

pub async fn stage(state: SelectChain, msg: SelectChainMsg, eff: Effects<SelectChainMsg>) -> SelectChain {
    match msg {
        SelectChainMsg::TipFromUpstream(tip) => state.handle_tip_from_upstream(tip, eff).await,
        SelectChainMsg::BlockValidationResult(point, valid) => {
            state.handle_block_validation_result(point, valid, eff).await
        }
    }
}

impl SelectChain {
    async fn handle_tip_from_upstream(mut self, tip: Tip, eff: Effects<SelectChainMsg>) -> SelectChain {
        let store = Store::new(eff.clone());

        let (Some(header), valid) = store.load_header_with_validity(&tip.hash()) else {
            tracing::error!(%tip, "tip not found");
            return eff.terminate().await;
        };

        if let Some(valid) = valid {
            tracing::warn!(%tip, %valid, "got tip from upstream that was already validated");
            return self;
        } else {
            tracing::debug!(%tip, "got new tip from upstream");
        }

        if let Some(parent) = header.parent_hash() {
            // if parent is in tips, extend that chain; otherwise check store for fragment
            if let Some(mut chain) = self.tips.remove(&parent) {
                tracing::debug!(%parent, %tip, "extending chain");
                chain.push(tip.hash());
                self.tips.insert(tip.hash(), chain);
            } else {
                // the message type allows that we get a Tip that was already sent earlier, the track_peers stage
                // shouldn’t send this, so we don’t defend against it here
                let mut valid = true;
                let mut extending = false;
                let mut ancestors = store
                    .ancestors_with_validity(parent)
                    .take_while(|(h, v)| {
                        if *v == Some(false) {
                            valid = false;
                            false
                        } else {
                            if self.tips.remove(&h.hash()).is_some() {
                                extending = true;
                            };
                            v.is_none()
                        }
                    })
                    .map(|(h, _)| h.hash())
                    .collect::<Vec<_>>();
                if valid {
                    let action = if extending { "extending" } else { "new" };
                    tracing::debug!(%parent, %tip, "{action} chain");
                    ancestors.reverse();
                    ancestors.push(tip.hash());
                    self.tips.insert(tip.hash(), ancestors);
                } else {
                    tracing::info!(%parent, %tip, "upstream tip depends on invalid block");
                }
            }
        } else {
            tracing::debug!(%tip, "new chain from origin");
            self.tips.insert(tip.hash(), vec![tip.hash()]);
        }

        if self.tips.contains_key(&tip.hash()) && cmp_tip(&tip, &self.best_tip) == Ordering::Greater {
            tracing::debug!(%tip, "new best tip candidate");
            self.best_tip = tip;
            eff.send(&self.downstream, tip).await;
        }
        self
    }

    async fn handle_block_validation_result(
        mut self,
        point: Point,
        valid: bool,
        eff: Effects<SelectChainMsg>,
    ) -> SelectChain {
        let store = Store::new(eff.clone());
        if !store.has_header(&point.hash()) {
            tracing::error!(%point, "header not found while trying to store block validation result");
            return eff.terminate().await;
        }

        store
            .set_block_valid(&point.hash(), valid)
            .or_terminate(&eff, async |error| {
                tracing::error!(%error, %valid, "failed to store block validation result");
            })
            .await;

        if valid {
            // TODO: add anchor management and pruning of tips that are older than new anchor
            let h = point.hash();
            self.tips.values_mut().for_each(|v| {
                if v.first() == Some(&h) {
                    v.remove(0);
                }
                // block validation results should arrive in order, from oldest to newest
                debug_assert!(!v.contains(&h));
            });
        } else {
            // remove all tips depending on the invalid block
            // (if a peer sends further headers on this chain, we will ignore them)
            let prev_tips = self.tips.len();
            self.tips.retain(|_k, v| v.first() != Some(&point.hash()));
            let removed = prev_tips - self.tips.len();

            if !self.tips.contains_key(&self.best_tip.hash()) {
                tracing::info!(%removed, "best tip candidate invalidated");
                // need to pick new best tip
                self.best_tip = self
                    .tips
                    .keys()
                    .filter_map(|h| store.load_header(h).map(|h| h.tip()))
                    .max_by(cmp_tip)
                    .unwrap_or_else(|| {
                        store.load_header(&store.get_best_chain_hash()).map(|h| h.tip()).unwrap_or(Tip::origin())
                    });
                if self.best_tip != Tip::origin() {
                    self.tips.entry(self.best_tip.hash()).or_insert(vec![]);
                }
                tracing::debug!(%self.best_tip, "new best tip candidate");
                eff.send(&self.downstream, self.best_tip).await;
            } else if removed > 0 {
                tracing::warn!(%removed, "chain fork(s) removed due to invalid block");
            }
        }
        self
    }
}

/// Compare tips according to the rules for selecting the better chain.
///
/// This is core to the logic of this file, so even if it matched the `Ord` instance for `Tip`, it is
/// presented here for clarity.
fn cmp_tip(a: &Tip, b: &Tip) -> Ordering {
    a.block_height().cmp(&b.block_height()).then_with(|| b.slot().cmp(&a.slot()))
}

#[cfg(test)]
mod test_setup;
#[cfg(test)]
mod tests;
