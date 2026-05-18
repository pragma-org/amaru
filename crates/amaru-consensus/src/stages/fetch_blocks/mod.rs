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

use std::time::Duration;

use amaru_kernel::{
    BlockHeader, BlockHeight, HeaderHash, IsHeader, ORIGIN_HASH, Peer, Point, Tip, cardano::network_block::NetworkBlock,
};
use amaru_observability::TraceContext;
use amaru_ouroboros_traits::{MissingBlocks, MissingBlocksResult};
use amaru_protocols::{blockfetch::Blocks2, manager::ManagerMessage, store_effects::Store};
use pure_stage::{Effects, OrTerminateWith, ScheduleId, StageRef, TryInStage};
use tracing::Instrument;

use crate::stages::{
    block_source::BlockSourceMsg,
    peer_selection::PeerSelectionMsg,
    select_chain::{SelectChainMsg, TipCandidate, load_parent_point},
    validate_block::ValidateBlockMsg,
};

// TODO make configurable
const MAX_MISSING_BLOCKS_PER_BATCH: usize = 25;

/// Block fetch coordinator stage.
///
/// This stage drives the retrieval of full blocks for headers that have been selected
/// by the upstream select_chain stage. It computes missing block ranges via the store,
/// requests them from the network via the manager (using the block-fetch protocol),
/// handles arrivals (with straggler protection), stores them, and forwards them
/// downstream for validation while advancing the chain selection loop. It also
/// handles startup recovery of blocks that were downloaded but not yet validated
/// before a prior shutdown.
///
/// ## Overview
/// - Pure actor stage (see `stage()` fn) processing `FetchBlocksMsg`.
/// - Collaborates with a dynamically ensured child stage (`cleanup_replies`) to safely
///   handle out-of-order or late block replies without clogging its own mailbox.
/// - Uses bounded batches (MAX_MISSING_BLOCKS_PER_BATCH=25) for fetch requests.
/// - Timeout-driven retry (5s) with upstream signaling for continuation.
///
/// ## Input messages and behaviour
/// - `NewTip(tip, parent)`: Update tracked block_height, assert no outstanding missing,
///   delegate to `request_missing_blocks` which queries store for gaps and (if any)
///   sends `ManagerMessage::FetchBlocks2` (with cr=child ref for replies) then schedules
///   a `Timeout(req_id)`.
/// - `RecoverStoredBlocks(best_hash)`: Startup recovery only. If origin, signal upstream
///   immediately. Otherwise replay any unvalidated-but-stored blocks downstream for
///   re-validation (using unvalidated_ancestor_hashes + has_block checks), falling back
///   to `request_missing_blocks` on first gap. Terminates on store errors.
/// - `Block(peer, network_block)`: Decode + basic integrity (body_hash match → adversarial
///   on fail), ordering checks against current `missing` cursor (parent + first point;
///   stragglers logged and dropped). On match: store block, send `(Tip, parent_boundary, block_height)`
///   downstream, `shift_one_block` on cursor; if now empty, clear state, cancel timeout,
///   signal `FetchNextFrom` upstream.
/// - `Timeout(req_id)`: If matches current, log error, clear missing/timeout, signal
///   `FetchNextFrom(boundary)` upstream to retry (no direct peer penalty here).
///
/// ## Child stages and their protocols
/// - **cleanup_replies** (dynamic, `StageRef<Blocks2>`, lazily `ensure_child`'d on every
///   message; factory creates `Cleanup` with self-ref + block_source/peer_selection):
///   - Receives `Blocks2` replies routed by manager (because `cr` passed in FetchBlocks2).
///   - `NoBlocks(_)`: ignored (timeout will handle).
///   - `Block(id, peer, nb)`: decode header (adversarial on fail + return), ALWAYS
///     `BlockSourceMsg::BlockReceived {peer, tip}` (for stats/selection), forward as
///     `FetchBlocksMsg::Block` to parent ONLY if id >= curr_id (straggler filter),
///     update curr_id = max.
///   - `Done(id)`: advance curr_id to id+1 max (to ignore subsequent old msgs from prior req).
///   - Purpose (per doc): "Ensure that straggling block replies do not clog the mailbox of the fetch stage."
///   - In prod starts as blackhole; replaced on first use. Tests inject named mock via `for_tests`.
///
/// ## Key state (missing blocks, requests, timeouts)
/// - `downstream: StageRef<(Tip, Point, BlockHeight)>`: where validated-ready blocks go (contramapped in wiring).
/// - `req_id: u64`: monotonic, incremented on each new FetchBlocks2; used to pair timeouts and filter in child.
/// - `missing: Option<MissingBlocks>`: cursor over current batch (from `find_missing_blocks`); supports
///   `from_to()`, `first()`, `boundary()`, `shift_one_block()`, `is_empty()`, `nb_missing_blocks()`.
///   Asserted None on NewTip/Recover entry; cleared on completion, timeout, or no-work cases.
/// - `timeout: Option<ScheduleId>`: the pending 5s timeout for current req; taken/cancelled only on batch success.
/// - `block_height: BlockHeight`: monotonic max over seen tips; passed with every downstream send (for both live and recovered blocks).
/// - Other refs: upstream (for FetchNextFrom continuation), manager (requests), block_source (receipts via child), peer_selection (adversarial reports).
///
/// ## External interactions (which stages it talks to)
/// - **select_chain (upstream)**: receives `NewTip`/`RecoverStoredBlocks`; sends `SelectChainMsg::FetchNextFrom(point)` on batch done, no-work, timeout, or recovery complete.
/// - **manager**: sends `ManagerMessage::FetchBlocks2 {from, through, id, cr: cleanup_replies}` to initiate block fetches (replies flow back via provided child ref).
/// - **downstream** (typically validate_block_input via contramap in build): sends `(Tip, parent_Point, block_height)` for each block (newly fetched or recovered stored).
/// - **block_source** (via child only): `BlockReceived {peer, tip}` for every header seen in replies (even stragglers/old).
/// - **peer_selection**: `Adversarial(peer)` on body-hash mismatch (main) or header decode failure (child).
/// - **Store** (via effects): `find_missing_blocks`, `has_block`, `load_header`, `store_block`, `unvalidated_ancestor_hashes`, `load_tip` etc. (many via `or_terminate`).
/// - Time/schedule via Effects: `schedule_after` for `Timeout`, `cancel_schedule`.
/// - No direct interaction with validate results (one-way downstream); no header validation performed here (assumes requested ranges; minimal structural checks only).
///
/// The `stage()` fn ensures the child then dispatches the 4 msg variants to the impl methods and returns updated state. All error paths that cannot continue call `eff.terminate()`.

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FetchBlocks {
    downstream: StageRef<ValidateBlockMsg>,
    req_id: u64,
    missing: Option<MissingBlocks>,
    upstream: StageRef<SelectChainMsg>,
    manager: StageRef<ManagerMessage>,
    block_source: StageRef<BlockSourceMsg>,
    peer_selection: StageRef<PeerSelectionMsg>,
    cleanup_replies: StageRef<Blocks2>,
    timeout: Option<ScheduleId>,
    block_height: BlockHeight,
    /// Overall context triggering a call to this stage
    #[serde(skip, default)]
    consensus_context: TraceContext,
    /// Temporary context used when making a call to fetch blocks
    #[serde(skip, default)]
    fetch_blocks_context: TraceContext,
}

impl FetchBlocks {
    pub fn new(
        downstream: StageRef<ValidateBlockMsg>,
        upstream: StageRef<SelectChainMsg>,
        manager: StageRef<ManagerMessage>,
        block_source: StageRef<BlockSourceMsg>,
        peer_selection: StageRef<PeerSelectionMsg>,
    ) -> Self {
        Self {
            downstream,
            req_id: 0,
            missing: None,
            upstream,
            manager,
            block_source,
            peer_selection,
            cleanup_replies: StageRef::blackhole(),
            timeout: None,
            block_height: BlockHeight::from(0),
            consensus_context: TraceContext::none(),
            fetch_blocks_context: TraceContext::none(),
        }
    }

    /// Constructor for tests: use a mock cleanup_replies stage instead of wiring the real one.
    #[cfg(test)]
    pub fn for_tests(
        downstream: StageRef<ValidateBlockMsg>,
        upstream: StageRef<SelectChainMsg>,
        manager: StageRef<ManagerMessage>,
        block_source: StageRef<BlockSourceMsg>,
        peer_selection: StageRef<PeerSelectionMsg>,
        cleanup_replies: StageRef<Blocks2>,
    ) -> Self {
        Self {
            downstream,
            req_id: 0,
            missing: None,
            upstream,
            manager,
            block_source,
            peer_selection,
            cleanup_replies,
            timeout: None,
            block_height: BlockHeight::from(0),
            consensus_context: TraceContext::none(),
            fetch_blocks_context: TraceContext::none(),
        }
    }

    pub async fn new_tip(&mut self, tip_candidate: TipCandidate, eff: Effects<FetchBlocksMsg>) {
        let TipCandidate { tip, parent, context } = tip_candidate;
        self.block_height = tip.block_height().max(self.block_height);

        tracing::debug!(tip = %tip.point(), parent = %parent, "fetching blocks");
        assert!(
            self.missing.is_none(),
            "there shouldn't be any missing blocks when starting a new tip: {:?}",
            self.missing
        );

        self.request_missing_blocks(tip, parent, eff, &context).await;
    }

    /// Startup-only recovery: resubmit downloaded blocks whose validity was not
    /// persisted before shutdown, then fetch from the first missing block.
    pub async fn recover_stored_blocks(
        &mut self,
        eff: Effects<FetchBlocksMsg>,
        best_hash: HeaderHash,
        context: TraceContext,
    ) {
        assert!(
            self.missing.is_none(),
            "there shouldn't be any missing blocks when recovering stored blocks: {:?}",
            self.missing
        );

        let store = Store::new(eff.clone());
        if best_hash == ORIGIN_HASH {
            eff.send(&self.upstream, SelectChainMsg::FetchNextFrom(Point::Origin, context.clone())).await;
            return;
        }
        let best_tip = store
            .load_header(&best_hash)
            .await
            .or_terminate(&eff, async move |_| {
                tracing::error!(hash = %best_hash, "cannot load header for best candidate");
            })
            .await;
        let unvalidated = store.unvalidated_ancestor_hashes(best_hash).await.0;

        self.block_height = best_tip.block_height().max(self.block_height);
        let tip = best_tip.tip();

        tracing::debug!(tip = %tip.point(), "recovering stored blocks");

        let mut parent: Option<Point> = None;
        for hash in unvalidated {
            let Some(header) = store.load_header(&hash).await else {
                tracing::error!(%hash, "failed to load candidate header");
                return eff.terminate().await;
            };
            let tip = header.tip();
            let block_parent = match parent {
                Some(p) => p,
                None => load_parent_point(&eff, &store, &header).await,
            };
            match store.has_block(&hash).await {
                Ok(true) => {
                    tracing::debug!(point = %tip.point(), "validating stored block");
                    eff.send(
                        &self.downstream,
                        ValidateBlockMsg::new(tip, block_parent, self.block_height, context.clone()),
                    )
                    .await;
                    parent = Some(tip.point());
                }
                Ok(false) => {
                    self.request_missing_blocks(tip, block_parent, eff, &context).await;
                    return;
                }
                Err(error) => {
                    tracing::error!(%error, %hash, "failed to check stored block");
                    return eff.terminate().await;
                }
            }
        }

        tracing::info!(tip = %tip.point(), "no blocks to fetch");
        eff.send(&self.upstream, SelectChainMsg::FetchNextFrom(tip.point(), context.clone())).await;
    }

    async fn request_missing_blocks(
        &mut self,
        tip: Tip,
        parent: Point,
        eff: Effects<FetchBlocksMsg>,
        context: &TraceContext,
    ) {
        let store = Store::new(eff.clone());
        match store.find_missing_blocks(tip.hash(), MAX_MISSING_BLOCKS_PER_BATCH).await {
            Ok(MissingBlocksResult::StartHeaderNotFound) => {
                tracing::error!("failed to load initial header");
                return eff.terminate().await;
            }
            Ok(MissingBlocksResult::BoundaryNotFound) => {
                tracing::debug!("no boundary for missing blocks found given the new tip");
                self.missing = None;
            }
            Ok(MissingBlocksResult::Found(missing_blocks)) => {
                self.missing = Some(missing_blocks);
            }
            Err(error) => {
                tracing::error!(%error, "failed to find missing blocks");
                return eff.terminate().await;
            }
        }
        let Some(missing) = self.missing.as_ref() else {
            return;
        };

        match missing.from_to() {
            None => {
                self.missing = None;
                tracing::info!(tip = %tip.point(), parent = %parent, "no blocks to fetch");
                eff.send(&self.upstream, SelectChainMsg::FetchNextFrom(tip.point(), context.clone())).await;
            }
            Some((from, to)) => {
                let from = *from;
                let to = *to;
                let length = missing.nb_missing_blocks();
                tracing::debug!(%from, %to, length, "requesting blocks");
                self.req_id += 1;
                let fetch_blocks_span = amaru_observability::trace_span!(
                    parent_context: &context,
                    amaru_observability::amaru::consensus::FETCH_BLOCKS,
                    from = from.to_string(),
                    through = to.to_string(),
                    from_slot = from.slot_or_default().as_u64(),
                    through_slot = to.slot_or_default().as_u64(),
                    missing_blocks = length,
                    request_id = self.req_id
                );
                self.consensus_context = context.clone();
                self.fetch_blocks_context = TraceContext::from_span(&fetch_blocks_span);
                eff.send(
                    &self.manager,
                    ManagerMessage::FetchBlocks2 {
                        from,
                        through: to,
                        id: self.req_id,
                        cr: self.cleanup_replies.clone(),
                        context: self.fetch_blocks_context.clone(),
                    },
                )
                .await;
                let timeout = eff.schedule_after(FetchBlocksMsg::Timeout(self.req_id), Duration::from_secs(5)).await;
                self.timeout = Some(timeout);
            }
        }
    }

    pub async fn block(&mut self, peer: Peer, network_block: NetworkBlock, eff: Effects<FetchBlocksMsg>) {
        let store = Store::new(eff.clone());
        let block = match network_block.decode_block() {
            Ok(block) => block,
            Err(error) => {
                tracing::error!(%error, "failed to decode block");
                return;
            }
        };
        let header = BlockHeader::from(&block.header);
        let point = header.point();
        tracing::debug!(%point, "received block");

        let process_span = amaru_observability::trace_span!(
            parent_context: &self.fetch_blocks_context,
            amaru_observability::amaru::consensus::PROCESS_BLOCK,
            hash = point.hash(),
            slot = point.slot_or_default().as_u64(),
        );
        let process_context = TraceContext::from_span(&process_span);
        async move {
            // check that body belongs to header
            if header.header().header_body.block_body_hash != block.body_hash() {
                eff.send(&self.peer_selection, PeerSelectionMsg::Adversarial(peer)).await;
                tracing::warn!(expected = %header.header().header_body.block_body_hash, actual = %block.body_hash(), "block body hash mismatch");
                return;
            }
            let Some(missing) = self.missing.as_mut() else {
                tracing::debug!(%peer, "received straggler block");
                return;
            };
            if header.parent_hash() != Some(missing.boundary().hash()) {
                // this happens for stragglers when fetching from multiple peers
                tracing::debug!(expected = %missing.boundary().hash(), actual = %header.parent_hash().unwrap_or(ORIGIN_HASH), "block parent hash mismatch");
                return;
            }
            if Some(point) != missing.first() {
                let expected = missing.first().map(|p| p.to_string()).unwrap_or("none".to_string());
                tracing::warn!(%expected, actual = ?point, "block point mismatch");
                return;
            }

            store
                .store_block(&point.hash(), &network_block.raw_block(), &process_context)
                .or_terminate_with(&eff, async |error| {
                    tracing::error!(%error, "failed to store block");
                })
                .await;
            let tip = Tip::new(point, block.header.header_body.block_number.into());
            eff.send(&self.downstream, ValidateBlockMsg::new(tip, missing.boundary(), self.block_height, self.consensus_context.clone())).await;

            missing.shift_one_block();
            if missing.is_empty() {
                self.missing = None;
                if let Some(timeout) = self.timeout.take() {
                    eff.cancel_schedule(timeout).await;
                }
                // we have now received all the blocks we were expecting to receive
                self.fetch_blocks_context.clear();
                eff.send(&self.upstream, SelectChainMsg::FetchNextFrom(point, self.consensus_context.clone())).await;
            }
        }.instrument(process_span).await
    }

    pub async fn timeout(&mut self, req_id: u64, eff: Effects<FetchBlocksMsg>) {
        if req_id != self.req_id {
            return;
        }
        tracing::error!(%req_id, "timeout fetching blocks");
        match self.missing.as_ref().map(|m| m.boundary()) {
            None => (),
            Some(from) => {
                self.timeout = None;
                self.missing = None;
                self.fetch_blocks_context.clear();
                eff.send(&self.upstream, SelectChainMsg::FetchNextFrom(from, self.consensus_context.clone())).await;
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum FetchBlocksMsg {
    NewTip(TipCandidate),
    RecoverStoredBlocks(HeaderHash, #[serde(skip, default)] TraceContext),
    Block(Peer, NetworkBlock),
    Timeout(u64),
}

impl FetchBlocksMsg {
    pub fn message_type(&self) -> &'static str {
        match self {
            FetchBlocksMsg::NewTip(_) => "NewTip",
            FetchBlocksMsg::RecoverStoredBlocks(..) => "RecoverStoredBlocks",
            FetchBlocksMsg::Block(_, _) => "Block",
            FetchBlocksMsg::Timeout(_) => "Timeout",
        }
    }

    pub fn context(&self) -> TraceContext {
        match self {
            FetchBlocksMsg::NewTip(tc) => tc.context.clone(),
            FetchBlocksMsg::RecoverStoredBlocks(_, ctx) => ctx.clone(),
            FetchBlocksMsg::Block(_, _) | FetchBlocksMsg::Timeout(_) => TraceContext::none(),
        }
    }
}

pub async fn stage(mut state: FetchBlocks, msg: FetchBlocksMsg, eff: Effects<FetchBlocksMsg>) -> FetchBlocks {
    eff.ensure_child(&mut state.cleanup_replies, "cleanup_replies", cleanup_replies, || {
        Cleanup::new(eff.me(), state.block_source.clone(), state.peer_selection.clone())
    })
    .await;
    match msg {
        FetchBlocksMsg::NewTip(tip_candidate) => state.new_tip(tip_candidate, eff).await,
        FetchBlocksMsg::RecoverStoredBlocks(best_hash, ctx) => state.recover_stored_blocks(eff, best_hash, ctx).await,
        FetchBlocksMsg::Block(peer, block) => state.block(peer, block, eff).await,
        FetchBlocksMsg::Timeout(req_id) => state.timeout(req_id, eff).await,
    }
    state
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct Cleanup {
    curr_id: u64,
    fetch: StageRef<FetchBlocksMsg>,
    block_source: StageRef<BlockSourceMsg>,
    peer_selection: StageRef<PeerSelectionMsg>,
}

impl Cleanup {
    fn new(
        fetch: StageRef<FetchBlocksMsg>,
        block_source: StageRef<BlockSourceMsg>,
        peer_selection: StageRef<PeerSelectionMsg>,
    ) -> Self {
        Self { curr_id: 0, fetch, block_source, peer_selection }
    }
}

pub fn recover_stored_blocks_msg(best_hash: HeaderHash) -> FetchBlocksMsg {
    let span = amaru_observability::trace_span!(
        amaru_observability::amaru::consensus::RECOVER_STORED_BLOCKS,
        hash = best_hash
    );
    FetchBlocksMsg::RecoverStoredBlocks(best_hash, TraceContext::from_span(&span))
}

/// Ensure that straggling block replies do not clog the mailbox of the fetch stage.
///
/// TODO: keep block hashes in LRU to deduplicate incoming blocks without validation or ordering assumption
async fn cleanup_replies(mut state: Cleanup, msg: Blocks2, eff: Effects<Blocks2>) -> Cleanup {
    match msg {
        // completely ignore empty responses, fetch stage will deal with timeouts
        Blocks2::NoBlocks(_) => {}
        Blocks2::Block(id, peer, network_block, context) => {
            let header = match network_block.decode_header() {
                Ok(header) => header,
                Err(error) => {
                    tracing::warn!(%error, "failed to decode block in cleanup");
                    eff.send(&state.peer_selection, PeerSelectionMsg::Adversarial(peer)).await;
                    return state;
                }
            };
            eff.send(
                &state.block_source,
                BlockSourceMsg::BlockReceived { peer: peer.clone(), tip: header.tip(), context },
            )
            .await;
            if id >= state.curr_id {
                eff.send(&state.fetch, FetchBlocksMsg::Block(peer, network_block)).await;
            }
            // getting higher id implies a new request has started
            state.curr_id = id.max(state.curr_id);
        }
        // getting done message implies a new request will start with id+1, but Done might be old as well
        Blocks2::Done(id) => {
            state.curr_id = (id + 1).max(state.curr_id);
        }
    }
    state
}

#[cfg(test)]
mod test_setup;
#[cfg(test)]
mod tests;
