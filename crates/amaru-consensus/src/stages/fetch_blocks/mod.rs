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
use amaru_observability::{TraceContext, debug_span};
use amaru_ouroboros_traits::{MissingBlocks, MissingBlocksResult};
use amaru_protocols::{blockfetch::Blocks, manager::ManagerMessage, store_effects::Store};
use amaru_pure_stage::{Effects, OrTerminateWith, ScheduleId, StageRef, TryInStage};
use tracing::Instrument;

use crate::stages::{block_source::BlockSourceMsg, peer_selection::PeerSelectionMsg, select_chain::SelectChainMsg};

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
///   sends `ManagerMessage::FetchBlocks` (with cr=child ref for replies) then schedules
///   a `Timeout(req_id)`.
/// - `RecoverStoredBlocks { from, to }`: Startup recovery only, where `from` is the ledger tip and
///   `to` the best stored candidate. Walks the stored headers from `to` back down to `from` and
///   replays them downstream for re-validation (using `ancestors_between` + `has_block` checks),
///   falling back to `request_missing_blocks` on first gap. Terminates on store errors, and on an
///   origin `from`, which means the ledger was never bootstrapped.
/// - `Block(peer, network_block)`: Decode + basic integrity (body_hash match → adversarial
///   on fail), ordering checks against current `missing` cursor (parent + first point;
///   stragglers logged and dropped). On match: store block, send `(Tip, parent_boundary, block_height)`
///   downstream, `shift_one_block` on cursor; if now empty, clear state, cancel timeout,
///   signal `FetchNextFrom` upstream.
/// - `Timeout(req_id)`: If matches current, log error (unless paused for no peers), clear
///   missing/timeout, signal `FetchNextFrom(boundary)` upstream to retry (no direct peer penalty here).
/// - `NoPeersAvailable(req_id)`: If matches current, log INFO that fetch is paused; leave the
///   5s timeout armed so retry is rate-limited without ERROR.
///
/// ## Child stages and their protocols
/// - **cleanup_replies** (dynamic, `StageRef<Blocks>`, lazily `ensure_child`'d on every
///   message; factory creates `Cleanup` with self-ref + block_source/peer_selection):
///   - Receives `Blocks` replies routed by manager (because `cr` passed in FetchBlocks).
///   - `NoBlocks(_)`: ignored (timeout will handle).
///   - `NoPeersAvailable(id)`: forward `FetchBlocksMsg::NoPeersAvailable(id)` to parent.
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
/// - `req_id: u64`: monotonic, incremented on each new FetchBlocks; used to pair timeouts and filter in child.
/// - `missing: Option<MissingBlocks>`: cursor over current batch (from `find_missing_blocks`); supports
///   `from_to()`, `first()`, `boundary()`, `shift_one_block()`, `is_empty()`, `nb_missing_blocks()`.
///   Asserted None on NewTip/Recover entry; cleared on completion, timeout, or no-work cases.
/// - `timeout: Option<ScheduleId>`: the pending 5s timeout for current req; taken/cancelled only on batch success.
/// - `no_peers_pause: bool`: set when manager reports no initiating connections; suppresses ERROR on the following timeout.
/// - `block_height: BlockHeight`: monotonic max over seen tips; passed with every downstream send (for both live and recovered blocks).
/// - Other refs: upstream (for FetchNextFrom continuation), manager (requests), block_source (receipts via child), peer_selection (adversarial reports).
///
/// ## External interactions (which stages it talks to)
/// - **select_chain (upstream)**: receives `NewTip`/`RecoverStoredBlocks`; sends `SelectChainMsg::FetchNextFrom(point)` on batch done, no-work, timeout, or recovery complete.
/// - **manager**: sends `ManagerMessage::FetchBlocks {from, through, id, cr: cleanup_replies}` to initiate block fetches (replies flow back via provided child ref).
/// - **downstream** (typically validate_block_input via contramap in build): sends `(Tip, parent_Point, block_height)` for each block (newly fetched or recovered stored).
/// - **block_source** (via child only): `BlockReceived {peer, tip}` for every header seen in replies (even stragglers/old).
/// - **peer_selection**: `Adversarial(peer)` on body-hash mismatch (main) or header decode failure (child).
/// - **Store** (via effects): `find_missing_blocks`, `has_block`, `load_header`, `store_block`, `load_tip` etc. (many via `or_terminate`).
/// - Time/schedule via Effects: `schedule_after` for `Timeout`, `cancel_schedule`.
/// - No direct interaction with validate results (one-way downstream); no header validation performed here (assumes requested ranges; minimal structural checks only).
///
/// The `stage()` fn ensures the child then dispatches the 4 msg variants to the impl methods and returns updated state. All error paths that cannot continue call `eff.terminate()`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FetchBlocks {
    downstream: StageRef<DownloadedBlock>,
    req_id: u64,
    missing: Option<MissingBlocks>,
    upstream: StageRef<SelectChainMsg>,
    manager: StageRef<ManagerMessage>,
    block_source: StageRef<BlockSourceMsg>,
    peer_selection: StageRef<PeerSelectionMsg>,
    cleanup_replies: StageRef<Blocks>,
    timeout: Option<ScheduleId>,
    /// Set when the manager reports no initiating peers; suppresses ERROR on the next timeout.
    no_peers_pause: bool,
    block_height: BlockHeight,
    /// Trace context originating from the reception of a new tip. Additional spans created by
    /// this stage are children of that context
    trace_context: Option<TraceContext>,
}

impl FetchBlocks {
    pub fn new(
        downstream: StageRef<DownloadedBlock>,
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
            no_peers_pause: false,
            block_height: BlockHeight::from(0),
            trace_context: Default::default(),
        }
    }

    /// Constructor for tests: use a mock cleanup_replies stage instead of wiring the real one.
    #[cfg(test)]
    pub fn for_tests(
        downstream: StageRef<DownloadedBlock>,
        upstream: StageRef<SelectChainMsg>,
        manager: StageRef<ManagerMessage>,
        block_source: StageRef<BlockSourceMsg>,
        peer_selection: StageRef<PeerSelectionMsg>,
        cleanup_replies: StageRef<Blocks>,
    ) -> Self {
        let fetch_blocks = FetchBlocks::new(downstream, upstream, manager, block_source, peer_selection);
        Self { cleanup_replies, ..fetch_blocks }
    }

    pub async fn new_tip(
        &mut self,
        tip: Tip,
        parent: Point,
        eff: Effects<FetchBlocksMsg>,
        parent_context: TraceContext,
    ) {
        self.block_height = tip.block_height().max(self.block_height);

        tracing::debug!(tip = %tip.point(), parent = %parent, "fetching blocks");
        assert!(
            self.missing.is_none(),
            "there shouldn't be any missing blocks when starting a new tip: {:?}",
            self.missing
        );

        let span = debug_span!(
            parent_context: parent_context.clone(),
            consensus::blocks::FETCH,
            tip = tip,
            header_hash = tip.hash(),
        );
        let stage_context = (&span).into();

        self.request_missing_blocks(tip, parent, eff, parent_context, stage_context).instrument(span).await;
    }

    /// Startup-only recovery: resubmit the blocks stored between the ledger tip `from` and the best
    /// candidate `to`, so the ledger can apply them again, then fetch from the first missing block.
    ///
    /// The ledger has no persisted volatile state, so `from` is where it resumes and therefore the
    /// only parent the first replayed block may have.
    pub async fn recover_stored_blocks(&mut self, eff: Effects<FetchBlocksMsg>, from: Point, to: HeaderHash) {
        let span = debug_span!(consensus::blocks::RECOVER_STORED, from = from, to = to);
        let trace_context = (&span).into();
        assert!(
            self.missing.is_none(),
            "there shouldn't be any missing blocks when recovering stored blocks: {:?}",
            self.missing
        );

        let store = Store::new(eff.clone()).with_trace_context(&trace_context);

        // An origin ledger tip means that we are trying to recover from an empty ledger with
        // a non-empty store. This is a misconfiguration, and we should not attempt to replay the stored headers.
        if from.hash() == ORIGIN_HASH {
            tracing::error!(%from, %to, "inconsistent data: stored headers found while the ledger has no tip");
            return eff.terminate().await;
        }

        let Some(to_replay) = store.ancestors_between(from, to).await else {
            tracing::error!(%from, %to, "the stored headers do not form a chain down to the ledger tip");
            return eff.terminate().await;
        };

        let best_tip_header = store
            .load_header(&to)
            .await
            .or_terminate(&eff, async move |_| {
                tracing::error!(hash = %to, "cannot load header for best candidate");
            })
            .await;

        self.block_height = best_tip_header.block_height().max(self.block_height);
        let tip = best_tip_header.point();
        tracing::debug!(%tip, "recovering stored blocks");

        // Replay blocks one by done. The replay will have downloaded blocks as a prefix, then
        // missing blocks. The first ones are retrieved from the chain store and the other ones
        // are fetched from peers
        let mut parent = from;
        let mut to_fetch = Vec::new();
        let mut path = to_replay.into_iter();

        while let Some(block_tip) = path.next() {
            let hash = block_tip.hash();
            match store.has_block(&hash).await {
                Ok(true) => {
                    tracing::debug!(point = %block_tip.point(), "validating stored block");
                    let downloaded_block = DownloadedBlock {
                        tip: block_tip,
                        parent,
                        max_block_height: self.block_height,
                        trace_context: trace_context.clone(),
                    };
                    eff.send(&self.downstream, downloaded_block).await;
                    parent = block_tip.point();
                }
                // Nothing beyond this point was ever downloaded, since blocks are only ever stored in
                // ancestor order. The rest of the path is therefore exactly what remains to fetch.
                Ok(false) => {
                    to_fetch.push(block_tip.point());
                    to_fetch.extend(path.by_ref().map(|block_tip| block_tip.point()));
                    to_fetch.truncate(MAX_MISSING_BLOCKS_PER_BATCH);
                    break;
                }
                Err(error) => {
                    tracing::error!(%error, %hash, "failed to check stored block");
                    return eff.terminate().await;
                }
            }
        }

        // `parent` is the last block handed over for validation, so it is the boundary the fetched
        // blocks must chain onto. An empty batch means the replay covered the whole path, in which case
        // this just tells the upstream stage to carry on.
        let missing = MissingBlocks::new(parent, to_fetch);
        self.request_blocks(missing, best_tip_header.tip(), parent, eff, trace_context).await;
    }

    /// Find the oldest missing blocks in the chain ending with `tip` and fetch them.
    async fn request_missing_blocks(
        &mut self,
        tip: Tip,
        parent: Point,
        eff: Effects<FetchBlocksMsg>,
        parent_context: TraceContext,
        stage_context: TraceContext,
    ) {
        let store = Store::new(eff.clone()).with_trace_context(&stage_context);
        let missing = match store.find_missing_blocks(tip.hash(), MAX_MISSING_BLOCKS_PER_BATCH).await {
            Ok(MissingBlocksResult::StartHeaderNotFound) => {
                tracing::error!("failed to load initial header");
                return eff.terminate().await;
            }
            Ok(MissingBlocksResult::BoundaryNotFound) => {
                tracing::debug!("no boundary for missing blocks found given the new tip");
                self.missing = None;
                return;
            }
            Ok(MissingBlocksResult::Found(missing_blocks)) => missing_blocks,
            Err(error) => {
                tracing::error!(%error, "failed to find missing blocks");
                return eff.terminate().await;
            }
        };

        self.request_blocks(missing, tip, parent, eff, parent_context).await
    }

    /// Ask peers for a batch of blocks already known to be missing, or tell the upstream stage to
    /// carry on when the batch turns out to be empty.
    async fn request_blocks(
        &mut self,
        missing: MissingBlocks,
        tip: Tip,
        parent: Point,
        eff: Effects<FetchBlocksMsg>,
        parent_context: TraceContext,
    ) {
        let Some((from, through)) = missing.from_to().map(|(from, through)| (*from, *through)) else {
            self.missing = None;
            tracing::info!(tip = %tip.point(), parent = %parent, "no blocks to fetch");
            return self.fetch_next_from(eff, tip.point()).await;
        };

        tracing::debug!(%from, %through, length = missing.nb_missing_blocks(), "requesting blocks");
        self.req_id += 1;
        self.no_peers_pause = false;
        self.trace_context = Some(parent_context);

        let now = eff.clock().await;
        let requested: Vec<HeaderHash> = missing.missing_points().into_iter().map(|point| point.hash()).collect();
        self.missing = Some(missing);

        eff.send(
            &self.manager,
            ManagerMessage::FetchBlocks { from, through, id: self.req_id, cr: self.cleanup_replies.clone() },
        )
        .await;
        // Tell the select_chain stage when this block was received so it can record the
        // block_fetch_wait point of its lifecycle.
        eff.send(&self.upstream, SelectChainMsg::BlocksRequested(requested, now)).await;
        let timeout = eff.schedule_after(FetchBlocksMsg::Timeout(self.req_id), Duration::from_secs(5)).await;
        self.timeout = Some(timeout);
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

        // check that body belongs to header
        if header.header().header_body.block_body_hash != block.body_hash() {
            let span = debug_span!(consensus::block::MISMATCHED_HASH, peer = peer.clone(), header_hash = point.hash());
            eff.send(&self.peer_selection, PeerSelectionMsg::Adversarial(peer, (&span).into())).await;
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

        // Tell the select_chain stage when this block was received so it can record the
        // block_downloaded point of its lifecycle.
        let now = eff.clock().await;
        eff.send(&self.upstream, SelectChainMsg::BlockDownloaded(point.hash(), now)).await;

        store
            .store_block(&point.hash(), &network_block.raw_block())
            .or_terminate_with(&eff, async |error| {
                tracing::error!(%error, "failed to store block");
            })
            .await;
        let tip = Tip::new(point, block.header.header_body.block_number.into());

        // retrieve the trace context that led to fetching that block to send downstream
        let trace_context = self.trace_context.clone().unwrap_or_default();

        let downloaded_block =
            DownloadedBlock { tip, parent: missing.boundary(), max_block_height: self.block_height, trace_context };
        eff.send(&self.downstream, downloaded_block).await;

        missing.shift_one_block();
        if missing.is_empty() {
            self.missing = None;
            self.no_peers_pause = false;
            if let Some(timeout) = self.timeout.take() {
                eff.cancel_schedule(timeout).await;
            }
            self.fetch_next_from(eff, point).await;
        }
    }

    pub async fn no_peers_available(&mut self, req_id: u64, _eff: Effects<FetchBlocksMsg>) {
        if req_id != self.req_id || self.missing.is_none() {
            return;
        }

        tracing::info!(%req_id, "block fetching paused due to no upstream peers");
        self.no_peers_pause = true;
    }

    pub async fn timeout(&mut self, req_id: u64, eff: Effects<FetchBlocksMsg>) {
        if req_id != self.req_id {
            return;
        }

        if self.no_peers_pause {
            tracing::debug!(%req_id, "retrying block fetch after no-peers pause");
        } else {
            tracing::error!(%req_id, "timeout fetching blocks");
        }
        match self.missing.as_ref().map(|m| m.boundary()) {
            None => (),
            Some(from) => {
                self.timeout = None;
                self.missing = None;
                self.no_peers_pause = false;
                self.fetch_next_from(eff, from).await;
            }
        }
    }

    async fn fetch_next_from(&mut self, eff: Effects<FetchBlocksMsg>, from: Point) {
        let trace_context = self.trace_context.take().unwrap_or_default();
        eff.send(&self.upstream, SelectChainMsg::FetchNextFrom(from, trace_context)).await;
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DownloadedBlock {
    pub tip: Tip,
    pub parent: Point,
    pub max_block_height: BlockHeight,
    pub trace_context: TraceContext,
}

impl DownloadedBlock {
    pub fn new(tip: Tip, parent: Point, max_block_height: BlockHeight) -> Self {
        Self { tip, parent, max_block_height, trace_context: Default::default() }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum FetchBlocksMsg {
    NewTip { tip: Tip, parent: Point, trace_context: TraceContext },
    RecoverStoredBlocks { from: Point, to: HeaderHash, trace_context: TraceContext },
    Block(Peer, NetworkBlock),
    Timeout(u64),
    NoPeersAvailable(u64),
}

impl FetchBlocksMsg {
    pub fn new_tip(tip: Tip, parent: Point) -> Self {
        Self::NewTip { tip, parent, trace_context: Default::default() }
    }

    pub fn recover_stored_blocks(from: Point, to: HeaderHash) -> Self {
        Self::RecoverStoredBlocks { from, to, trace_context: Default::default() }
    }
}

pub async fn stage(mut state: FetchBlocks, msg: FetchBlocksMsg, eff: Effects<FetchBlocksMsg>) -> FetchBlocks {
    eff.ensure_child(&mut state.cleanup_replies, "cleanup_replies", cleanup_replies, || {
        Cleanup::new(eff.me(), state.block_source.clone(), state.peer_selection.clone())
    })
    .await;
    match msg {
        FetchBlocksMsg::NewTip { tip, parent, trace_context } => state.new_tip(tip, parent, eff, trace_context).await,
        FetchBlocksMsg::RecoverStoredBlocks { from, to, trace_context } => {
            state.trace_context = Some(trace_context);
            state.recover_stored_blocks(eff, from, to).await
        }
        FetchBlocksMsg::Block(peer, block) => state.block(peer, block, eff).await,
        FetchBlocksMsg::Timeout(req_id) => state.timeout(req_id, eff).await,
        FetchBlocksMsg::NoPeersAvailable(req_id) => state.no_peers_available(req_id, eff).await,
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

/// Ensure that straggling block replies do not clog the mailbox of the fetch stage.
///
/// TODO: keep block hashes in LRU to deduplicate incoming blocks without validation or ordering assumption
async fn cleanup_replies(mut state: Cleanup, msg: Blocks, eff: Effects<Blocks>) -> Cleanup {
    match msg {
        // completely ignore empty responses, fetch stage will deal with timeouts
        Blocks::NoBlocks(_) => {}
        Blocks::NoPeersAvailable(id) => {
            eff.send(&state.fetch, FetchBlocksMsg::NoPeersAvailable(id)).await;
        }
        Blocks::Block(id, peer, network_block) => {
            let header = match network_block.decode_header() {
                Ok(header) => header,
                Err(error) => {
                    tracing::warn!(%error, "failed to decode block in cleanup");
                    eff.send(&state.peer_selection, PeerSelectionMsg::adversarial(peer)).await;
                    return state;
                }
            };
            eff.send(&state.block_source, BlockSourceMsg::BlockReceived { peer: peer.clone(), tip: header.tip() })
                .await;
            if id >= state.curr_id {
                eff.send(&state.fetch, FetchBlocksMsg::Block(peer, network_block)).await;
            }
            // getting higher id implies a new request has started
            state.curr_id = id.max(state.curr_id);
        }
        // getting done message implies a new request will start with id+1, but Done might be old as well
        Blocks::Done(id) => {
            state.curr_id = (id + 1).max(state.curr_id);
        }
    }
    state
}

#[cfg(test)]
mod test_setup;
#[cfg(test)]
mod tests;
