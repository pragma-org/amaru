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

use amaru_kernel::{BlockHeader, HeaderHash, IsHeader, ORIGIN_HASH, Point, Tip};
use amaru_protocols::store_effects::Store;
use anyhow::anyhow;
use pure_stage::{Effects, OrTerminateWith, StageRef};

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
    pub fn new(downstream: StageRef<(Tip, Point)>) -> Self {
        Self { downstream, best_tip: None, tips: BTreeMap::new(), may_fetch_blocks: false }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SelectChainMsg {
    Initialize,
    TipFromUpstream(Tip, Point),
    BlockValidationResult(Tip, bool),
    // This message must also be preloaded upon startup to get the block-fetching
    // and validation processes started. Should then contain Point::Origin.
    FetchNextFrom(Point),
}

pub async fn stage(mut state: SelectChain, msg: SelectChainMsg, eff: Effects<SelectChainMsg>) -> SelectChain {
    match msg {
        SelectChainMsg::Initialize => {
            let store = Store::new(eff);
            let best_tip = best_tip_from_store(&store).await.unwrap_or(None);
            state.best_tip = best_tip.map(|(best_tip, to_validate)| {
                state.tips.insert(best_tip.hash(), to_validate);
                best_tip
            });
            state
        }
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

        let Some((header, valid)) = store.load_header_with_validity(&tip.hash()).await else {
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
            let (mut ancestors, valid) = store.unvalidated_ancestor_hashes(parent.hash()).await;
            if valid {
                tracing::debug!(%parent, tip = %tip.point(), "new chain");
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
        if !store.has_header(&tip.hash()).await {
            tracing::error!(%tip, "header not found while trying to store block validation result");
            return eff.terminate().await;
        }

        store
            .set_block_valid(&tip.hash(), valid)
            .or_terminate_with(&eff, async |error| {
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
            self.tips.retain(|_k, v| v.first() != Some(&tip.hash()));
            let removed = prev_tips - self.tips.len();

            if let Some(best_tip) = &self.best_tip
                && !self.tips.contains_key(&best_tip.hash())
            {
                tracing::info!(%removed, "best tip candidate invalidated");
                // need to pick new best tip
                match best_tip_from_store(&store).await {
                    Ok(Some((new_best_tip, to_validate))) => {
                        tracing::debug!(%new_best_tip, "new best tip candidate");
                        let parent = load_parent_point(&eff, store, &new_best_tip).await;
                        if self.may_fetch_blocks {
                            self.may_fetch_blocks = false;
                            eff.send(&self.downstream, (new_best_tip.tip(), parent)).await;
                        }
                        self.tips.insert(new_best_tip.hash(), to_validate);
                        self.best_tip = Some(new_best_tip);
                    }
                    Ok(None) => {
                        self.best_tip = None;
                        tracing::warn!("falling back to origin");
                    }
                    Err(e) => {
                        tracing::error!("{e:?}");
                        return eff.terminate().await;
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
            let store = Store::new(eff.clone());
            let header = store
                .load_header(&best_tip.hash())
                .or_terminate_with(&eff, async |_| {
                    tracing::error!("failed to load header of best candidate");
                })
                .await;
            let parent = if let Some(parent) = header.parent_hash() {
                store
                    .load_tip(&parent)
                    .or_terminate_with(&eff, async |_| {
                        tracing::error!("failed to load parent of best candidate");
                    })
                    .await
                    .point()
            } else {
                Point::Origin
            };
            tracing::debug!(tip = %best_tip.point(), %parent, "resuming block fetching");
            eff.send(&self.downstream, (best_tip.tip(), parent)).await;
        } else {
            self.may_fetch_blocks = true;
        }
        self
    }
}

/// Return the highest tip candidate that extends the current best chain, together with
/// the unknown suffix after the last validated ancestor.
///
/// Example:
///
/// O--A(valid)--B(valid)  best_chain_hash
///                     \
///                      C(?)--D(?)         candidate 1
///                      \
///                       E(?)--F(invalid)--G(?)  candidate 2, rejected
///                       \
///                       H(?)                candidate 3
///
/// The candidates are the reachable leaves below `best_chain_hash`: `D`, `G`, and `H`.
/// The branch ending at `G` is rejected because it contains `F(invalid)`.
/// Between `D` and `H`, the highest tip wins according to `cmp_tip`.
///
/// If `D` wins, this returns:
/// `(D, [C, D])` since `C` and `D` have not-yet-validated blocks.
pub async fn best_tip_from_store(store: &Store) -> anyhow::Result<Option<(BlockHeader, Vec<HeaderHash>)>> {
    let best_chain_hash = store.get_best_chain_hash().await;
    if best_chain_hash != ORIGIN_HASH && store.load_header_with_validity(&best_chain_hash).await.is_none() {
        return Err(anyhow!("best chain hash {best_chain_hash} does not resolve to a stored header"));
    }

    let mut best_candidate = None;

    // ORIGIN_HASH cannot have a block, so we start from its direct children
    let mut to_visit = if best_chain_hash == ORIGIN_HASH {
        store.get_children(&best_chain_hash).await.into_iter().collect()
    } else {
        // If the best chain hash is not origin, we have a first best candidate.
        best_candidate = store.load_header(&best_chain_hash).await;
        vec![best_chain_hash]
    };

    while let Some(hash) = to_visit.pop() {
        // Visit one reachable header.
        tracing::debug!(hash = %hash, "visiting startup candidate");
        let (header, validity) = store
            .load_header_with_validity(&hash)
            .await
            .ok_or_else(|| anyhow!("reachable child hash {hash} does not resolve to a stored header"))?;

        // Don't use this header if its block is invalid
        if validity == Some(false) {
            continue;
        };

        // Continue the traversal with all direct descendants of the current header.
        let children = store.get_children(&hash).await;

        if children.is_empty()
            && best_candidate.as_ref().is_none_or(|current| cmp_tip(Some(&header), Some(current)).is_gt())
        {
            best_candidate = Some(header);
        } else {
            // Propagate the updated suffix state to each child branch.
            to_visit.extend(children);
        }
    }

    if let Some(best_candidate) = best_candidate {
        let (best_missing, is_valid) = store.unvalidated_ancestor_hashes(best_candidate.hash()).await;
        return if is_valid { Ok(Some((best_candidate, best_missing))) } else { Ok(None) };
    }
    Ok(None)
}

/// Return the point of the parent of `header`, or `Point::Origin` if it has no parent.
/// The parent header must be present in the store otherwise the stage is terminated.
async fn load_parent_point(eff: &Effects<SelectChainMsg>, store: Store, header: &BlockHeader) -> Point {
    if let Some(parent) = header.parent() {
        store
            .load_tip(&parent)
            .or_terminate_with(eff, async |_| {
                tracing::warn!("failed to load parent {:?} of {:?}", parent, header);
            })
            .await
            .point()
    } else {
        Point::Origin
    }
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
mod test_setup;
#[cfg(test)]
mod tests;
