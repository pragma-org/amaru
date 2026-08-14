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

use amaru_kernel::{BlockHeight, Header, HeaderHash, IsHeader, ORIGIN_HASH, Point};
use amaru_observability::{TraceContext, debug_span};
use amaru_ouroboros::vrf;
use amaru_protocols::store_effects::Store;
use amaru_pure_stage::{Effects, OrTerminateWith, StageRef};
use tracing::Instrument;

use crate::{effects::FindBestCandidate, performance::Performance};

/// Chain selection / fork choice stage.
///
/// This stage is responsible for:
/// - Tracking pending unvalidated tip candidates (forks) received from upstream (track_peers).
/// - Selecting the "best" candidate according to Ouroboros Praos rules (see `cmp_tip`).
/// - Deciding when to push a `(Point)` to the downstream fetch/validation pipeline.
/// - Reacting to block validation results to advance or prune candidates.
/// - Bootstrapping from the persistent chain store on startup.
///
/// It is wired with a single `downstream: StageRef<(Point)>` (the input to the fetch_blocks stage).
/// The stage is driven purely by messages; it uses `Store` effects (via `Effects`) for header/validity/ancestor queries
/// and one external effect (`FindBestCandidate`) when the current best is invalidated.
///
/// Key internal state:
/// - `tips`: maps known tip hashes to the (oldest-first) list of yet-to-be-validated ancestor hashes on that branch.
/// - `best_tip`: the current best `Header` (or `None`); updated only on strictly better candidates per `cmp_tip`.
/// - `may_fetch_blocks`: "Whether the downstream stage has sent a FetchNextFrom message that has not yet been responded to."
///   Starts `false` (see `new()`); controls whether a newly superior tip triggers an immediate send.
///
/// This stage must be preloaded at graph construction time with `Initialize(...)` (current best from store); if there is
/// no FetchBlocks stage connected (e.g. in tests) it also needs `FetchNextFrom(Point::Origin)` (to kick off fetching) —
/// see `SelectChainMsg` docs and `test_setup::setup`.
///
/// ## Input message effects
///
/// - **Initialize(best_hash: HeaderHash)**:
///   - If `best_hash != ORIGIN_HASH`: loads the header (sets as `best_tip`), queries `unvalidated_ancestor_hashes`,
///     and inserts the result into `tips`. (No send to downstream; `may_fetch_blocks` untouched.)
///   - Otherwise: no-op (leaves empty state).
///   - Always returns the (mutated) state. Preloaded at startup for non-empty stores.
///
/// - **TipFromUpstream(tip: Point, parent: Point)**:
///   - Loads the header+validity via store (terminates with error log if the header is missing).
///   - Already-validated or already-invalid tips are dropped (debug/info); this stage must not
///     terminate on a pre-existing validity flag, because startup recovery may set block validity
///     after `track_peers` sent `NewTip` and before this message is processed. Already-tracked tips
///     (same hash already in `tips` or equal to `best_tip`) are a pure no-op.
///   - Otherwise inserts/extends a `tips` entry: special-cases `parent == Origin`; extends an existing
///     pending chain; or (for new forks) queries store ancestors (only if the prefix is valid) and
///     inserts the fragment. Ignores (with info log) tips depending on invalid blocks.
///   - If the new tip is now tracked in `tips` *and* is strictly better than `best_tip` per `cmp_tip`:
///     logs "new best tip candidate"; **if** `may_fetch_blocks` then sets it `false` and `eff.send(&downstream, (tip, parent))`;
///     unconditionally updates `best_tip`.
///   - (When `!may_fetch_blocks`, a better tip updates state silently; downstream learns only on its next `FetchNextFrom`.)
///
/// - **BlockValidationResult(point: Point, valid: bool, max_block_height: BlockHeight)**:
///   - Terminates (error) if header not present in store.
///   - Persists the validity result via `set_block_valid` (terminates on store error).
///   - If `valid`: for every pending chain, drains the prefix up through the now-validated hash (advances all branches).
///   - If `!valid`: prunes every `tips` entry whose first (unvalidated) element is this hash; `removed` count logged at warn if >0.
///     If the current `best_tip` is no longer present in `tips` (i.e., its branch was pruned): logs "best tip candidate invalidated",
///     calls `eff.external(FindBestCandidate)`, and on success (non-origin): loads the new header + parent point,
///     **if** `may_fetch_blocks` then sets `false` + sends `(new_best.point(), parent)` to downstream, inserts its unvalidated ancestors into `tips`,
///     and sets `best_tip`. Falls back to `best_tip = None` (warn "falling back to origin") on ORIGIN result; terminates on external error.
///   - (Validation results from the downstream validate_block stage are the primary way pending chains make forward progress or are discarded.)
///
/// - **FetchNextFrom(point: Point)**:
///   - Computes current `best_tip.point()` (or Origin).
///   - If a `best_tip` exists and differs from the requested `point`: loads its header + parent point (via `load_tip`/`load_parent_point` paths; terminates on failure),
///     logs "resuming block fetching", and **unconditionally** `eff.send(&downstream, (best_tip.point(), parent))`.
///     (Note: does **not** touch `may_fetch_blocks`.)
///   - Else (no best, or point matches current best): sets `may_fetch_blocks = true` (signals "we will push future superior tips").
///   - Comment notes this is preloaded at startup (typically Origin) to start the fetch/validate pipeline even for non-empty stores.
///   - This is the primary mechanism by which `may_fetch_blocks` is set `true`; the flag is only cleared immediately before the two "new best arrived" send sites.
///
/// The stage also defines supporting items used internally:
/// - `load_parent_point(...)`: helper to compute a header's parent `Point` (or Origin), terminating on missing parent.
/// - `cmp_tip(...)`: the core fork-choice comparator (height primary; VRF/opcert/slot≤5 tiebreakers per the linked Ouroboros Praos spec). Extensively unit-tested in `tests.rs`.
///
/// All sends to downstream use the exact payload [`NewBestTip`]. The stage makes heavy use of `Store::new(eff)` + `or_terminate_with` for resilience.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SelectChain {
    downstream: StageRef<NewBestTip>,
    /// Maps all block tree tips to the list of headers whose blocks are yet to be validated
    /// (oldest first)
    tips: BTreeMap<HeaderHash, Vec<HeaderHash>>,
    /// The best tip candidate, if any; is None for empty store.
    best_tip: Option<Header>,
    /// Whether the downstream stage has sent a FetchNextFrom message that has not yet been responded to.
    may_fetch_blocks: bool,
}

impl SelectChain {
    pub fn new(downstream: StageRef<NewBestTip>) -> Self {
        Self { downstream, best_tip: None, tips: BTreeMap::new(), may_fetch_blocks: false }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SelectChainMsg {
    Initialize(HeaderHash),
    TipFromUpstream { tip: Point, parent: Point, trace_context: TraceContext },
    BlockValidationResult(Point, bool, BlockHeight),
    // This message must also be preloaded upon startup to get the block-fetching
    // and validation processes started. Should then contain Point::Origin.
    FetchNextFrom(Point, TraceContext),
}

impl SelectChainMsg {
    pub fn tip_from_upstream(tip: Point, parent: Point) -> Self {
        SelectChainMsg::TipFromUpstream { tip, parent, trace_context: Default::default() }
    }

    pub fn fetch_next_from(point: Point) -> Self {
        SelectChainMsg::FetchNextFrom(point, Default::default())
    }
}

pub async fn stage(mut state: SelectChain, msg: SelectChainMsg, eff: Effects<SelectChainMsg>) -> SelectChain {
    match msg {
        SelectChainMsg::Initialize(best_hash) => {
            let store = Store::new(eff);
            if best_hash != ORIGIN_HASH {
                state.best_tip = store.load_header(&best_hash).await;
                // NOTE: validity has been checked by build_node already
                let (to_validate, _valid) = store.unvalidated_ancestor_hashes(best_hash).await;
                state.tips.insert(best_hash, to_validate);
            }
        }
        SelectChainMsg::TipFromUpstream { tip, parent, trace_context } => {
            let span = debug_span!(
                parent_context: &trace_context,
                consensus::chain::SELECT_FROM_TIP,
                tip = tip,
                header_hash = tip.hash(),
            );
            let child_trace_context = (&span).into();
            state.handle_tip_from_upstream(tip, parent, eff, trace_context, child_trace_context).instrument(span).await;
        }
        SelectChainMsg::BlockValidationResult(point, valid, max_block_height) => {
            let span = debug_span!(
                consensus::chain::SELECT_FROM_BLOCK_VALIDATION,
                point = point,
                valid = valid,
                header_hash = point.hash(),
            );
            let trace_context = (&span).into();
            state
                .handle_block_validation_result(point, valid, max_block_height, eff, trace_context)
                .instrument(span)
                .await;
        }
        SelectChainMsg::FetchNextFrom(point, trace_context) => {
            let span = debug_span!(parent_context: trace_context, consensus::chain::FETCH_NEXT, point = point, header_hash = point.hash(),);
            let trace_context = (&span).into();
            state.handle_fetch_next_from(point, eff, trace_context).instrument(span).await;
        }
    }
    state
}

impl SelectChain {
    /// Handle a tip from upstream.
    ///
    /// The `tip` and `parent` refer to headers that are guaranteed to be stored in the chain store
    /// by the track_peers stage.
    async fn handle_tip_from_upstream(
        &mut self,
        tip: Point,
        parent: Point,
        eff: Effects<SelectChainMsg>,
        parent_context: TraceContext,
        stage_context: TraceContext,
    ) {
        let store = Store::new(eff.clone()).with_trace_context(&stage_context);

        let Some((header, validity)) = store.load_header_with_validity(&tip.hash()).await else {
            tracing::error!(tip = %tip, "tip not found");
            return eff.terminate().await;
        };

        // Block-body validity is independent of header validation. During startup,
        // `fetch_blocks` recovery can set a validity flag after `track_peers` checked the
        // header and before this message is processed. Already-decided blocks are dropped:
        // recovery/`Initialize` already own that chain; re-adopting here would grow `tips`
        // and disturb best-tip bookkeeping for no gain.
        match validity {
            Some(true) => {
                tracing::debug!(tip = %tip, "ignoring tip from upstream that was already validated");
                return;
            }
            Some(false) => {
                tracing::info!(tip = %tip, "ignoring tip from upstream whose block is already invalid");
                let now = eff.clock().await;
                eff.external(Performance::record_header_abandoned(tip.hash(), now)).await;
                return;
            }
            None => {
                tracing::debug!(tip = %tip, "got new tip from upstream");
            }
        }

        // Redundant NewTip (e.g. concurrent peers announcing the same header) must be a no-op.
        if self.tips.contains_key(&tip.hash()) || self.best_tip.as_ref().is_some_and(|h| h.hash() == tip.hash()) {
            tracing::debug!(tip = %tip, "ignoring tip from upstream that is already tracked");
            return;
        }

        if parent == Point::Origin {
            tracing::debug!(tip = %tip, "new chain from origin");
            self.tips.insert(tip.hash(), vec![tip.hash()]);
        } else
        // if parent is in tips, extend that chain; otherwise check store for fragment
        if let Some(mut chain) = self.tips.remove(&parent.hash()) {
            tracing::debug!(%parent, tip = %tip, "extending chain");
            chain.push(tip.hash());
            self.tips.insert(tip.hash(), chain);
        } else {
            // New fork (or first tip on a path not yet tracked): the new branch can only be one
            // header long, but it may still require multiple block validations to reach a valid chain.
            let (mut ancestors, valid) = store.unvalidated_ancestor_hashes(parent.hash()).await;
            if valid {
                tracing::debug!(%parent, tip = %tip, "new chain");
                ancestors.push(tip.hash()); // new block must be validated by definition
                self.tips.insert(tip.hash(), ancestors);
            } else {
                tracing::info!(%parent, %tip, "upstream tip depends on invalid block");
                let now = eff.clock().await;
                eff.external(Performance::record_header_abandoned(tip.hash(), now)).await;
            }
        }

        if self.tips.contains_key(&tip.hash()) && cmp_tip(Some(&header), self.best_tip.as_ref()) == Ordering::Greater {
            let best_tip = self.best_tip.take().map(|h| h.point()).unwrap_or(Point::Origin);
            tracing::debug!(tip = %tip, previous = %best_tip, "new best tip candidate");

            // if we have a real fork, start recording the time it takes to switch to that fork
            if parent.hash() != best_tip.hash() {
                let now = eff.clock().await;
                eff.external(Performance::record_fork_started(tip, now)).await;
            }

            if self.may_fetch_blocks {
                self.may_fetch_blocks = false;
                eff.send(&self.downstream, NewBestTip { tip, parent, trace_context: parent_context }).await;
            }
            self.best_tip = Some(header);
        }
    }

    async fn handle_block_validation_result(
        &mut self,
        tip: Point,
        valid: bool,
        max_block_height: BlockHeight,
        eff: Effects<SelectChainMsg>,
        trace_context: TraceContext,
    ) {
        // While catching up, slot-start-to-header is not a meaningful network-health signal.
        let syncing = max_block_height > tip.block_height();
        let store = Store::new(eff.clone()).with_trace_context(&trace_context);
        if !store.has_header(&tip.hash()).await {
            tracing::error!(%tip, "header not found while trying to store block validation result");
            return eff.terminate().await;
        }

        let tip_hash = tip.hash();
        store
            .set_block_valid(&tip_hash, valid)
            .or_terminate_with(&eff, async |error| {
                tracing::error!(%error, %valid, "failed to store block validation result");
            })
            .await;

        if valid {
            self.tips.values_mut().for_each(|v| {
                if let Some(idx) = v.iter().position(|hash| hash == &tip_hash) {
                    v.drain(0..=idx);
                }
            });
            let now = eff.clock().await;
            eff.external(Performance::record_block_valid(tip_hash, now, syncing)).await;
            return;
        }
        // INVALID CASE
        //
        // remove all tips depending on the invalid block
        // (if a peer sends further headers on this chain, we will ignore them)
        let prev_tips = self.tips.len();
        let pruned: Vec<HeaderHash> =
            self.tips.extract_if(.., |_k, v| v.contains(&tip_hash)).flat_map(|(_, v)| v).collect();
        let removed = prev_tips - self.tips.len();

        let mut switched_to = None;
        if let Some(best_tip) = &self.best_tip
            && !self.tips.contains_key(&best_tip.hash())
        {
            tracing::info!(%removed, "best tip candidate invalidated");
            // need to pick new best tip
            match eff.external(FindBestCandidate).await {
                Ok(new_best_tip) if new_best_tip != ORIGIN_HASH => {
                    tracing::debug!(%new_best_tip, "new best tip candidate");
                    let new_best_tip = store
                        .load_header(&new_best_tip)
                        .or_terminate_with(&eff, async move |_| {
                            tracing::error!(hash = %new_best_tip, "best candidate does not exist");
                        })
                        .await;
                    let parent = load_parent_point(&eff, &store, &new_best_tip).await;
                    if self.may_fetch_blocks {
                        self.may_fetch_blocks = false;
                        eff.send(&self.downstream, NewBestTip { tip: new_best_tip.point(), parent, trace_context })
                            .await;
                    }
                    let (to_validate, _) = store.unvalidated_ancestor_hashes(new_best_tip.hash()).await;
                    // Only record a fork switch if there are blocks to validate on the new best tip.
                    // Otherwise, there are no blocks to apply and no switch occurs.
                    // This is typically the case when we have a tip that is invalid but one of its
                    // ancestors was applied succesfully to the ledger. Here we just revert to knowing
                    // that this ancestor is the best tip and we will continue to fetch blocks from it.
                    if !to_validate.is_empty() {
                        switched_to = Some(new_best_tip.point());
                    }
                    self.tips.insert(new_best_tip.hash(), to_validate);
                    self.best_tip = Some(new_best_tip);
                }
                Ok(_) => {
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

        // end the performance tracking for blocks that could not be successfully validated and that
        // we dropped because a better chain is available
        let now = eff.clock().await;
        for hash in &pruned {
            eff.external(Performance::record_block_pruned(*hash, hash == &tip_hash, now, syncing)).await;
        }

        // switching away from the invalidated best tip to a candidate with pending validations is a fork switch
        if let Some(new_tip) = switched_to {
            eff.external(Performance::record_fork_started(new_tip, now)).await;
        }
    }

    async fn handle_fetch_next_from(
        &mut self,
        point: Point,
        eff: Effects<SelectChainMsg>,
        trace_context: TraceContext,
    ) {
        assert!(!self.may_fetch_blocks, "received FetchNextFrom while not having responded to previous one");
        // During startup with non-empty chain store, best_tip will be different from origin and
        // the incoming `point` will be origin, leading to sending the best tip to the downstream stage.
        let best_tip = self.best_tip.as_ref().map(|h| h.point()).unwrap_or(Point::Origin);
        tracing::debug!(%point, %best_tip, "handle_fetch_next_from");
        if let Some(best_tip) = &self.best_tip
            && best_tip.point() != point
        {
            let store = Store::new(eff.clone()).with_trace_context(&trace_context);
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
            } else {
                Point::Origin
            };
            tracing::debug!(tip = %best_tip.point(), %parent, "resuming block fetching");
            eff.send(&self.downstream, NewBestTip { tip: best_tip.point(), parent, trace_context }).await;
        } else {
            self.may_fetch_blocks = true;
        }
    }
}

/// Message informing downstream consumers that a new best tip has been found.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NewBestTip {
    pub tip: Point,
    pub parent: Point,
    pub trace_context: TraceContext,
}

impl NewBestTip {
    pub fn new(tip: Point, parent: Point) -> Self {
        NewBestTip { tip, parent, trace_context: Default::default() }
    }
}

/// Return the point of the parent of `header`, or `Point::Origin` if it has no parent.
/// The parent header must be present in the store otherwise the stage is terminated.
pub async fn load_parent_point<T: Send + Sync + 'static>(eff: &Effects<T>, store: &Store, header: &Header) -> Point {
    if let Some(parent) = header.parent() {
        store
            .load_tip(&parent)
            .or_terminate_with(eff, async |_| {
                tracing::warn!("failed to load parent {:?} of {:?}", parent, header);
            })
            .await
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
/// This is core to the logic of this file, so even if it matched the `Ord` instance for `Point`, it is
/// presented here for clarity.
pub fn cmp_tip(a: Option<&Header>, b: Option<&Header>) -> Ordering {
    let (a, b) = match (a, b) {
        (None, None) => return Ordering::Equal,
        (None, Some(_)) => return Ordering::Less,
        (Some(_), None) => return Ordering::Greater,
        (Some(a), Some(b)) => (a, b),
    };
    a.block_height().cmp(&b.block_height()).then_with(|| {
        let a_leader = vrf::Derivation::Leader.derive_tagged_vrf_output(a.vrf_output());
        let b_leader = vrf::Derivation::Leader.derive_tagged_vrf_output(b.vrf_output());
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
