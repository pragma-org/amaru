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
    downstream: StageRef<(Tip, Point)>,
    /// Maps all block tree tips to the list of headers whose blocks are yet to be validated
    /// (oldest first)
    tips: BTreeMap<HeaderHash, Vec<HeaderHash>>,
    best_tip: Tip,
}

impl SelectChain {
    pub fn new(downstream: StageRef<(Tip, Point)>, best_tip: Tip) -> Self {
        let mut tips = BTreeMap::new();
        if best_tip != Tip::origin() {
            tips.insert(best_tip.hash(), vec![]);
        }
        Self { downstream, best_tip, tips }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SelectChainMsg {
    TipFromUpstream(Tip, Point),
    BlockValidationResult(Point, bool),
}

pub async fn stage(state: SelectChain, msg: SelectChainMsg, eff: Effects<SelectChainMsg>) -> SelectChain {
    match msg {
        SelectChainMsg::TipFromUpstream(tip, parent) => state.handle_tip_from_upstream(tip, parent, eff).await,
        SelectChainMsg::BlockValidationResult(point, valid) => {
            state.handle_block_validation_result(point, valid, eff).await
        }
    }
}

impl SelectChain {
    /// Handle a tip from upstream.
    ///
    /// The `tip` and `parent` refer to headers that are guaranteed to be stored in the chain store
    /// by the track_peers stage.
    async fn handle_tip_from_upstream(mut self, tip: Tip, parent: Point, eff: Effects<SelectChainMsg>) -> SelectChain {
        let store = Store::new(eff.clone());

        let Some((_header, valid)) = store.load_header_with_validity(&tip.hash()) else {
            tracing::error!(%tip, "tip not found");
            return eff.terminate().await;
        };

        if let Some(valid) = valid {
            // track_peers only sends a tip if the header was just stored, so it cannot be already validated
            tracing::error!(%tip, %valid, "got tip from upstream that was already validated");
            return eff.terminate().await;
        } else {
            tracing::debug!(%tip, "got new tip from upstream");
        }

        if parent == Point::Origin {
            tracing::debug!(%tip, "new chain from origin");
            self.tips.insert(tip.hash(), vec![tip.hash()]);
        } else
        // if parent is in tips, extend that chain; otherwise check store for fragment
        if let Some(mut chain) = self.tips.remove(&parent.hash()) {
            tracing::debug!(%parent, %tip, "extending chain");
            chain.push(tip.hash());
            self.tips.insert(tip.hash(), chain);
        } else {
            // the message type allows that we get a Tip that was already sent earlier, the track_peers stage
            // shouldn’t send this, so we don’t defend against it here
            let mut valid = true;
            let mut extending = false;
            let mut ancestors = store
                .ancestors_with_validity(parent.hash())
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

        if self.tips.contains_key(&tip.hash()) && cmp_tip(&tip, &self.best_tip) == Ordering::Greater {
            tracing::debug!(%tip, "new best tip candidate");
            self.best_tip = tip;
            eff.send(&self.downstream, (tip, parent)).await;
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
                let parent;
                (self.best_tip, parent) = self
                    .tips
                    .keys()
                    .filter_map(|h| store.load_header(h).map(|h| (h.tip(), h.parent_hash())))
                    .max_by(|a, b| cmp_tip(&a.0, &b.0))
                    .unwrap_or_else(|| {
                        store
                            .load_header(&store.get_best_chain_hash())
                            .map(|h| (h.tip(), h.parent_hash()))
                            .unwrap_or((Tip::origin(), None))
                    });
                if self.best_tip != Tip::origin() {
                    tracing::debug!(%self.best_tip, "new best tip candidate");
                    let parent = if let Some(parent) = parent {
                        store
                            .load_tip(&parent)
                            .or_terminate(store.eff(), async |_| {
                                tracing::error!("failed to load parent of best tip candidate");
                            })
                            .await
                            .point()
                    } else {
                        Point::Origin
                    };
                    eff.send(&self.downstream, (self.best_tip, parent)).await;
                    self.tips.entry(self.best_tip.hash()).or_insert(vec![]);
                } else {
                    tracing::warn!("falling back to origin");
                }
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
