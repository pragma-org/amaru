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

use amaru_kernel::{BlockHeader, HeaderHash, IsHeader, Point, Tip};
use amaru_ouroboros::{ChainStore, ReadOnlyChainStore};
use amaru_protocols::store_effects::Store;
use pure_stage::{Effects, StageRef, TryInStage};

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SelectChain {
    downstream: StageRef<(Tip, Point)>,
    /// Maps all block tree tips to the list of headers whose blocks are yet to be validated
    /// (oldest first)
    tips: BTreeMap<HeaderHash, Vec<HeaderHash>>,
    /// The best tip candidate, if any; is None for empty store.
    best_tip: Option<BlockHeader>,
    /// Whether the downstream stage has sent a FetchNextFrom message that has not yet been responded to.
    may_fetch_blocks: bool,
}

impl SelectChain {
    pub fn new(downstream: StageRef<(Tip, Point)>, best_tip: Option<(BlockHeader, Vec<HeaderHash>)>) -> Self {
        let mut tips = BTreeMap::new();
        let best_tip = best_tip.map(|(best_tip, to_validate)| {
            tips.insert(best_tip.hash(), to_validate);
            best_tip
        });
        Self { downstream, best_tip, tips, may_fetch_blocks: false }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SelectChainMsg {
    TipFromUpstream(Tip, Point),
    BlockValidationResult(Tip, bool),
    // This message must also be preloaded upon startup to get the block-fetching
    // and validation processes started. Should then contain Point::Origin.
    FetchNextFrom(Point),
}

pub async fn stage(state: SelectChain, msg: SelectChainMsg, eff: Effects<SelectChainMsg>) -> SelectChain {
    match msg {
        SelectChainMsg::TipFromUpstream(tip, parent) => state.handle_tip_from_upstream(tip, parent, eff).await,
        SelectChainMsg::BlockValidationResult(point, valid) => {
            state.handle_block_validation_result(point, valid, eff).await
        }
        SelectChainMsg::FetchNextFrom(point) => state.handle_fetch_next_from(point, eff).await,
    }
}

impl SelectChain {
    /// Handle a tip from upstream.
    ///
    /// The `tip` and `parent` refer to headers that are guaranteed to be stored in the chain store
    /// by the track_peers stage.
    async fn handle_tip_from_upstream(mut self, tip: Tip, parent: Point, eff: Effects<SelectChainMsg>) -> SelectChain {
        let store = Store::new(eff.clone());

        let Some((header, valid)) = store.load_header_with_validity(&tip.hash()) else {
            tracing::error!(tip = %tip.point(), "tip not found");
            return eff.terminate().await;
        };

        if let Some(valid) = valid {
            // track_peers only sends a tip if the header was just stored, so it cannot be already validated
            tracing::error!(tip = %tip.point(), %valid, "got tip from upstream that was already validated");
            return eff.terminate().await;
        } else {
            tracing::debug!(tip = %tip.point(), "got new tip from upstream");
        }

        if parent == Point::Origin {
            tracing::debug!(tip = %tip.point(), "new chain from origin");
            self.tips.insert(tip.hash(), vec![tip.hash()]);
        } else
        // if parent is in tips, extend that chain; otherwise check store for fragment
        if let Some(mut chain) = self.tips.remove(&parent.hash()) {
            tracing::debug!(%parent, tip = %tip.point(), "extending chain");
            chain.push(tip.hash());
            self.tips.insert(tip.hash(), chain);
        } else {
            // since track_peers will only send newly stored tips, this is the case where
            // a new fork is detected; while the new fork can only be one header long, it
            // may still require multiple block validations to reach a valid chain
            if let Some(mut ancestors) = pending_non_invalidated_fragment(&store, parent.hash()) {
                tracing::debug!(%parent, tip = %tip.point(), "new chain");
                ancestors.reverse();
                ancestors.push(tip.hash()); // new block must be validated by definition
                self.tips.insert(tip.hash(), ancestors);
            } else {
                tracing::info!(%parent, %tip, "upstream tip depends on invalid block");
            }
        }

        if self.tips.contains_key(&tip.hash()) && cmp_tip(Some(&header), self.best_tip.as_ref()) == Ordering::Greater {
            let best_tip = self.best_tip.map(|h| h.point()).unwrap_or(Point::Origin);
            tracing::debug!(tip = %tip.point(), height = %tip.block_height(), %best_tip, "new best tip candidate");
            if self.may_fetch_blocks {
                self.may_fetch_blocks = false;
                eff.send(&self.downstream, (tip, parent)).await;
            }
            self.best_tip = Some(header);
        }
        self
    }

    async fn handle_block_validation_result(
        mut self,
        tip: Tip,
        valid: bool,
        eff: Effects<SelectChainMsg>,
    ) -> SelectChain {
        let store = Store::new(eff.clone());
        if !store.has_header(&tip.hash()) {
            tracing::error!(%tip, "header not found while trying to store block validation result");
            return eff.terminate().await;
        }

        store
            .set_block_valid(&tip.hash(), valid)
            .or_terminate(&eff, async |error| {
                tracing::error!(%error, %valid, "failed to store block validation result");
            })
            .await;

        if valid {
            let h = tip.hash();
            self.tips.values_mut().for_each(|v| {
                if let Some(idx) = v.iter().position(|hash| hash == &h) {
                    v.drain(0..=idx);
                }
            });
        } else {
            // remove all tips depending on the invalid block
            // (if a peer sends further headers on this chain, we will ignore them)
            let prev_tips = self.tips.len();
            self.tips.retain(|tip_hash, v| !v.contains(&tip.hash()) && !depends_on_hash(&store, *tip_hash, tip.hash()));
            let removed = prev_tips - self.tips.len();

            if let Some(best_tip) = &self.best_tip
                && !self.tips.contains_key(&best_tip.hash())
            {
                // Need to pick new best tip
                tracing::info!(%removed, "best tip candidate invalidated");

                match best_tip_candidate_from_store(&store) {
                    Some((new_best_tip, to_validate)) => {
                        tracing::debug!(%new_best_tip, "new best tip candidate");
                        let parent = if let Some(parent) = new_best_tip.parent() {
                            store
                                .load_tip(&parent)
                                .or_terminate(store.eff(), async |_| {
                                    tracing::warn!(
                                        "failed to load parent {:?} of best tip candidate {:?}",
                                        parent,
                                        new_best_tip
                                    );
                                })
                                .await
                                .point()
                        } else {
                            Point::Origin
                        };
                        if self.may_fetch_blocks {
                            self.may_fetch_blocks = false;
                            eff.send(&self.downstream, (new_best_tip.tip(), parent)).await;
                        }
                        self.tips.insert(new_best_tip.hash(), to_validate);
                        self.best_tip = Some(new_best_tip);
                    }
                    None => {
                        self.best_tip = None;
                        tracing::warn!("falling back to origin");
                    }
                }
            } else if removed > 0 {
                tracing::warn!(%removed, "chain fork(s) removed due to invalid block");
            }
        }
        self
    }

    async fn handle_fetch_next_from(mut self, point: Point, eff: Effects<SelectChainMsg>) -> SelectChain {
        // During startup with non-empty chain store, best_tip will be different from origin and
        // the incoming `point` will be origin, leading to sending the best tip to the downstream stage.
        let best_tip = self.best_tip.as_ref().map(|h| h.point()).unwrap_or(Point::Origin);
        tracing::debug!(%point, %best_tip, "handle_fetch_next_from");
        if let Some(best_tip) = &self.best_tip
            && best_tip.point() != point
        {
            let store = Store::new(eff);
            let header = store
                .load_header(&best_tip.hash())
                .or_terminate(store.eff(), async |_| {
                    tracing::error!("failed to load header of best candidate");
                })
                .await;
            let parent = if let Some(parent) = header.parent_hash() {
                store
                    .load_tip(&parent)
                    .or_terminate(store.eff(), async |_| {
                        tracing::error!("failed to load parent of best candidate");
                    })
                    .await
                    .point()
            } else {
                Point::Origin
            };
            tracing::debug!(tip = %best_tip.point(), %parent, "resuming block fetching");
            store.eff().send(&self.downstream, (best_tip.tip(), parent)).await;
        } else {
            self.may_fetch_blocks = true;
        }
        self
    }
}

/// Return the best non-invalidated tip from the chain store by taking descendants of the anchor and:
///
/// - Filter out tips that have invalid blocks.
/// - Take the best one using the cmp function.
/// - Also return the list of still non-validated block hashes for that tip.
///
pub fn best_tip_candidate_from_store(store: &dyn ChainStore<BlockHeader>) -> Option<(BlockHeader, Vec<HeaderHash>)> {
    store
        .child_tips(&store.get_anchor_hash())
        .filter_map(|tip| best_non_invalidated_candidate_from_leaf(store, tip.hash()))
        .max_by(|left, right| cmp_tip(Some(&left.0), Some(&right.0)))
}

/// Return the best candidate reachable on this leaf path together with the
/// still-pending fragment below the nearest validated ancestor.
///
/// The candidate is usually the leaf itself:
///
/// leaf (None)
///  |
/// parent (None)
///  |
/// validated (Some(true))
///
/// returns `(leaf, [parent, leaf])`
///
/// If the leaf suffix crosses an invalid block, the candidate collapses to the
/// best ancestor above that invalid segment:
///
/// leaf (None)
///  |
/// invalid (Some(false))
///  |
/// ancestor (None)
///  |
/// validated (Some(true))
///
/// returns `(ancestor, [ancestor])`
fn best_non_invalidated_candidate_from_leaf(
    chain_store: &dyn ChainStore<BlockHeader>,
    leaf_hash: HeaderHash,
) -> Option<(BlockHeader, Vec<HeaderHash>)> {
    let mut current = Some(leaf_hash);
    let mut pending_from_leaf = Vec::new();
    let mut candidate_header = None;
    let mut seen_valid_ancestor = false;

    while let Some(hash) = current {
        let (header, validity) = chain_store.load_header_with_validity(&hash)?;

        match validity {
            Some(false) => {
                pending_from_leaf.clear();
                candidate_header = None;
                seen_valid_ancestor = false;
            }
            Some(true) => {
                seen_valid_ancestor = true;
                candidate_header.get_or_insert(header.clone());
            }
            None => {
                if !seen_valid_ancestor {
                    pending_from_leaf.push(header.hash());
                }
                candidate_header.get_or_insert(header.clone());
            }
        }

        current = header.parent_hash();
    }

    candidate_header.map(|header| {
        let mut pending = pending_from_leaf;
        pending.reverse();
        (header, pending)
    })
}

/// Return the not-yet-validated fragment on the path from `hash` upward to the
/// nearest validated ancestor.
///
/// Return `None` if an invalid block is encountered before reaching one.
fn pending_non_invalidated_fragment(
    chain_store: &dyn ChainStore<BlockHeader>,
    hash: HeaderHash,
) -> Option<Vec<HeaderHash>> {
    let mut valid = true;
    let ancestors = chain_store
        .ancestors_with_validity(hash)
        .take_while(|(_header, validity)| {
            if *validity == Some(false) {
                valid = false;
                false
            } else {
                validity.is_none()
            }
        })
        .map(|(header, _)| header.hash())
        .collect::<Vec<_>>();

    valid.then_some(ancestors)
}

/// Return whether `hash` lies on a chain suffix whose ancestry includes `ancestor`.
fn depends_on_hash(store: &dyn ChainStore<BlockHeader>, hash: HeaderHash, ancestor: HeaderHash) -> bool {
    let mut current = Some(hash);

    while let Some(hash) = current {
        if hash == ancestor {
            return true;
        }

        current = store.load_header(&hash).and_then(|header| header.parent_hash());
    }

    false
}

/// Compare tip headers according to the rules for selecting the better chain.
///
/// <https://ouroboros-consensus.cardano.intersectmbo.org/pdfs/report.pdf#chapter.11>
/// <https://github.com/IntersectMBO/ouroboros-consensus/blob/57c3e32cafc13b9a5184e23fee057f5152eec03b/ouroboros-consensus-protocol/src/ouroboros-consensus-protocol/Ouroboros/Consensus/Protocol/Praos/Common.hs#L105-L120>
/// <https://github.com/IntersectMBO/ouroboros-consensus/blob/57c3e32cafc13b9a5184e23fee057f5152eec03b/ouroboros-consensus-protocol/src/ouroboros-consensus-protocol/Ouroboros/Consensus/Protocol/Praos/Common.hs#L188-L267>
/// <https://github.com/IntersectMBO/ouroboros-consensus/blob/main/ouroboros-consensus-cardano/src/shelley/Ouroboros/Consensus/Shelley/Ledger/Config.hs#L88-L94>
///
/// The rule to be implemented in Amaru is thus:
///
/// - prefer a candidate chain if it is longer
/// - prefer a candidate chain of equal length if the tip header’s VRF value is the same as ours and the opcert index is higher
/// - prefer a candidate chain of equal length if the tip header’s slot is at most 5 apart from ours and the VRF value is lower
/// - otherwise stick with the local candidate
///
/// This is core to the logic of this file, so even if it matched the `Ord` instance for `Tip`, it is
/// presented here for clarity.
pub fn cmp_tip(a: Option<&BlockHeader>, b: Option<&BlockHeader>) -> Ordering {
    let (a, b) = match (a, b) {
        (None, None) => return Ordering::Equal,
        (None, Some(_)) => return Ordering::Less,
        (Some(_), None) => return Ordering::Greater,
        (Some(a), Some(b)) => (a, b),
    };
    a.block_height().cmp(&b.block_height()).then_with(|| {
        let a_leader = a.vrf_leader();
        let b_leader = b.vrf_leader();
        if a_leader == b_leader {
            a.op_cert_seq().cmp(&b.op_cert_seq())
        } else if (a.slot() - b.slot()).abs() <= 5 {
            b_leader.cmp(&a_leader)
        } else {
            Ordering::Equal
        }
    })
}

#[cfg(test)]
mod property_tests;

#[cfg(test)]
mod test_setup;

#[cfg(test)]
mod tests;
