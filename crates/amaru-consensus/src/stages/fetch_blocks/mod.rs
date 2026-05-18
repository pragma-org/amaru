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

use std::{
    collections::BTreeMap,
    sync::{LazyLock, Mutex},
    time::Duration,
};

use amaru_kernel::{
    BlockHeader, BlockHeight, HeaderHash, IsHeader, ORIGIN_HASH, Point, Tip, cardano::network_block::NetworkBlock,
};
use amaru_ouroboros_traits::{MissingBlocks, MissingBlocksResult};
use amaru_protocols::{blockfetch::Blocks2, manager::ManagerMessage, store_effects::Store};
use pure_stage::{Effects, OrTerminateWith, ScheduleId, StageRef};
use tracing::{Instrument, Span};

use crate::{
    span::TraceContext,
    stages::{
        select_chain::{HeaderTrace, SelectChainMsg, best_tip_from_store, load_parent_point},
        validate_block::ValidateBlockMsg,
    },
};

// TODO make configurable
const MAX_MISSING_BLOCKS_PER_BATCH: usize = 25;

static FETCH_BLOCK_SPANS: LazyLock<Mutex<BTreeMap<HeaderHash, Span>>> = LazyLock::new(|| Mutex::new(BTreeMap::new()));

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FetchBlocks {
    downstream: StageRef<ValidateBlockMsg>,
    req_id: u64,
    missing: Option<MissingBlocks>,
    upstream: StageRef<SelectChainMsg>,
    manager: StageRef<ManagerMessage>,
    cleanup_replies: StageRef<Blocks2>,
    timeout: Option<ScheduleId>,
    block_height: BlockHeight,
    #[serde(skip, default)]
    header_context: TraceContext,
    #[serde(skip, default)]
    header_contexts: BTreeMap<amaru_kernel::HeaderHash, TraceContext>,
}

impl PartialEq for FetchBlocks {
    fn eq(&self, other: &Self) -> bool {
        self.downstream == other.downstream
            && self.req_id == other.req_id
            && self.missing == other.missing
            && self.upstream == other.upstream
            && self.manager == other.manager
            && self.cleanup_replies == other.cleanup_replies
            && self.timeout == other.timeout
            && self.block_height == other.block_height
    }
}

impl FetchBlocks {
    pub fn new(
        downstream: StageRef<ValidateBlockMsg>,
        upstream: StageRef<SelectChainMsg>,
        manager: StageRef<ManagerMessage>,
    ) -> Self {
        Self {
            downstream,
            req_id: 0,
            missing: None,
            upstream,
            manager,
            cleanup_replies: StageRef::blackhole(),
            timeout: None,
            block_height: BlockHeight::from(0),
            header_context: TraceContext::none(),
            header_contexts: BTreeMap::new(),
        }
    }

    /// Constructor for tests: use a mock cleanup_replies stage instead of wiring the real one.
    #[cfg(test)]
    pub fn for_tests(
        downstream: StageRef<ValidateBlockMsg>,
        upstream: StageRef<SelectChainMsg>,
        manager: StageRef<ManagerMessage>,
        cleanup_replies: StageRef<Blocks2>,
    ) -> Self {
        Self {
            downstream,
            req_id: 0,
            missing: None,
            upstream,
            manager,
            cleanup_replies,
            timeout: None,
            block_height: BlockHeight::from(0),
            header_context: TraceContext::none(),
            header_contexts: BTreeMap::new(),
        }
    }

    pub async fn new_tip(&mut self, header_trace: HeaderTrace, eff: Effects<FetchBlocksMsg>) {
        let HeaderTrace { tip, parent, context, contexts } = header_trace;
        self.block_height = tip.block_height().max(self.block_height);
        self.header_context = context;
        self.header_contexts = contexts;

        tracing::debug!(tip = %tip.point(), parent = %parent, "fetching blocks");
        assert!(
            self.missing.is_none(),
            "there shouldn't be any missing blocks when starting a new tip: {:?}",
            self.missing
        );

        self.request_missing_blocks(tip, parent, eff).await;
    }

    /// Startup-only recovery: resubmit downloaded blocks whose validity was not
    /// persisted before shutdown, then fetch from the first missing block.
    pub async fn recover_stored_blocks(&mut self, eff: Effects<FetchBlocksMsg>) {
        assert!(
            self.missing.is_none(),
            "there shouldn't be any missing blocks when recovering stored blocks: {:?}",
            self.missing
        );

        let store = Store::new(eff.clone());
        let (best_tip, unvalidated) = match best_tip_from_store(&store).await {
            Ok(Some(candidate)) => candidate,
            Ok(None) => {
                eff.send(&self.upstream, SelectChainMsg::FetchNextFrom(Point::Origin)).await;
                return;
            }
            Err(error) => {
                tracing::error!(%error, "failed to find best tip from store");
                return eff.terminate().await;
            }
        };

        self.block_height = best_tip.block_height().max(self.block_height);
        let tip = best_tip.tip();
        tracing::debug!(tip = %tip.point(), "recovering stored blocks");
        let recovery_context = recover_stored_blocks_context(tip);

        let mut parent: Option<Point> = None;
        for hash in unvalidated {
            let Some(header) = store.load_header(&hash).await else {
                tracing::error!(%hash, "failed to load candidate header");
                return eff.terminate().await;
            };
            let tip = header.tip();
            let block_parent = match parent {
                Some(p) => p,
                None => load_parent_point(&eff, store.clone(), &header).await,
            };
            match store.has_block(&hash).await {
                Ok(true) => {
                    tracing::debug!(point = %tip.point(), "validating stored block");
                    let context = recovered_process_block_context(&recovery_context, tip);
                    eff.send(&self.downstream, ValidateBlockMsg::new(tip, block_parent, self.block_height, context))
                        .await;
                    parent = Some(tip.point());
                }
                Ok(false) => {
                    self.request_missing_blocks(tip, block_parent, eff).await;
                    return;
                }
                Err(error) => {
                    tracing::error!(%error, %hash, "failed to check stored block");
                    return eff.terminate().await;
                }
            }
        }

        tracing::info!(tip = %tip.point(), "no blocks to fetch");
        eff.send(&self.upstream, SelectChainMsg::FetchNextFrom(tip.point())).await;
    }

    async fn request_missing_blocks(&mut self, tip: Tip, parent: Point, eff: Effects<FetchBlocksMsg>) {
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
            self.emit_skipped_fetch_block_span(tip, "boundary_not_found");
            return;
        };

        match missing.from_to() {
            None => {
                self.missing = None;
                self.emit_skipped_fetch_block_span(tip, "blocks_already_available");
                tracing::info!(tip = %tip.point(), parent = %parent, "no blocks to fetch");
                eff.send(&self.upstream, SelectChainMsg::FetchNextFrom(tip.point())).await;
            }
            Some((from, to)) => {
                let from = *from;
                let to = *to;
                let length = missing.nb_missing_blocks();
                let points = missing.points().collect::<Vec<_>>();
                tracing::debug!(%from, %to, length, "requesting blocks");
                self.start_fetch_block_spans(&points);
                self.req_id += 1;
                eff.send(
                    &self.manager,
                    ManagerMessage::FetchBlocks2 {
                        from,
                        through: to,
                        id: self.req_id,
                        cr: self.cleanup_replies.clone(),
                    },
                )
                .await;
                let timeout = eff.schedule_after(FetchBlocksMsg::Timeout(self.req_id), Duration::from_secs(5)).await;
                self.timeout = Some(timeout);
            }
        }
    }

    fn emit_skipped_fetch_block_span(&self, tip: Tip, reason: &str) {
        let hash = tip.hash();
        let context = self.header_contexts.get(&hash).unwrap_or(&self.header_context);
        let span = amaru_observability::trace_span!(
            parent_context: context,
            amaru_observability::amaru::consensus::FETCH_BLOCK,
            hash = hash,
            point = tip.point().to_string(),
            slot = tip.slot().as_u64(),
            fetch_block_skip_reason = reason.to_string()
        );
        let _guard = span.enter();
    }

    fn start_fetch_block_spans(&self, points: &[Point]) {
        for point in points {
            let context = self.header_contexts.get(&point.hash()).unwrap_or(&self.header_context);
            let span = amaru_observability::trace_span!(
                parent_context: context,
                amaru_observability::amaru::consensus::FETCH_BLOCK,
                hash = point.hash(),
                point = point.to_string(),
                slot = point.slot_or_default().as_u64()
            );
            if let Ok(mut spans) = FETCH_BLOCK_SPANS.lock() {
                spans.insert(point.hash(), span);
            }
        }
    }

    pub async fn block(&mut self, network_block: NetworkBlock, eff: Effects<FetchBlocksMsg>) {
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
        let tip = Tip::new(point, block.header.header_body.block_number.into());
        let context = self.header_contexts.get(&point.hash()).cloned().unwrap_or_else(|| self.header_context.clone());
        close_fetch_block_span(point.hash());
        let span = amaru_observability::trace_span!(
            parent_context: &context,
            amaru_observability::amaru::consensus::PROCESS_BLOCK,
            hash = point.hash(),
            point = point.to_string(),
            slot = point.slot_or_default().as_u64(),
            block_height = tip.block_height().as_u64()
        );
        let process_block_context = TraceContext::from_span(&span);

        async {
            tracing::debug!(%point, "received block");

            // check that body belongs to header
            if header.header().header_body.block_body_hash != block.body_hash() {
                tracing::warn!(expected = %header.header().header_body.block_body_hash, actual = %block.body_hash(), "block body hash mismatch");
                return;
            }
            let Some(missing) = self.missing.as_mut() else {
                // TODO: eventually accept blocks that could arrive when we don't get them within the timeout
                // provided that they are valid (parent block exists, no invalid parent).
                tracing::warn!("received block with no outstanding missing blocks");
                return;
            };
            if header.parent_hash() != Some(missing.boundary().hash()) {
                tracing::warn!(expected = %missing.boundary().hash(), actual = %header.parent_hash().unwrap_or(ORIGIN_HASH), "block parent hash mismatch");
                return;
            }
            if Some(point) != missing.first() {
                let expected = missing.first().map(|p| p.to_string()).unwrap_or("none".to_string());
                tracing::warn!(%expected, actual = ?point, "block point mismatch");
                return;
            }

            let boundary = missing.boundary();
            let downstream = self.downstream.clone();
            let block_height = self.block_height;
            store
                .store_block_with_context(&point.hash(), &network_block.raw_block(), process_block_context.clone())
                .or_terminate_with(&eff, async |error| {
                    tracing::error!(%error, "failed to store block");
                })
                .await;
            eff.send(&downstream, ValidateBlockMsg::new(tip, boundary, block_height, process_block_context)).await;

            missing.shift_one_block();
            if missing.is_empty() {
                self.missing = None;
                if let Some(timeout) = self.timeout.take() {
                    eff.cancel_schedule(timeout).await;
                }
                eff.send(&self.upstream, SelectChainMsg::FetchNextFrom(point)).await;
            }
        }
        .instrument(span)
        .await;
    }

    pub async fn timeout(&mut self, req_id: u64, eff: Effects<FetchBlocksMsg>) {
        if req_id != self.req_id {
            return;
        }
        tracing::error!(%req_id, "timeout fetching blocks");
        let missing_points = self.missing.as_ref().map(|missing| missing.points().collect::<Vec<_>>());
        match self.missing.as_ref().map(|m| m.boundary()) {
            None => (),
            Some(from) => {
                self.timeout = None;
                self.missing = None;
                if let Some(points) = missing_points {
                    close_fetch_block_spans(points.into_iter().map(|point| point.hash()));
                }
                eff.send(&self.upstream, SelectChainMsg::FetchNextFrom(from)).await;
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum FetchBlocksMsg {
    NewTip(HeaderTrace),
    RecoverStoredBlocks,
    Block(NetworkBlock),
    Timeout(u64),
}

pub async fn stage(mut state: FetchBlocks, msg: FetchBlocksMsg, eff: Effects<FetchBlocksMsg>) -> FetchBlocks {
    if state.cleanup_replies.is_blackhole() {
        let stage = eff.stage("cleanup_replies", cleanup_replies).await;
        state.cleanup_replies = eff.wire_up(stage, (0, eff.me())).await;
    }
    match msg {
        FetchBlocksMsg::NewTip(header_trace) => state.new_tip(header_trace, eff).await,
        FetchBlocksMsg::RecoverStoredBlocks => state.recover_stored_blocks(eff).await,
        FetchBlocksMsg::Block(block) => state.block(block, eff).await,
        FetchBlocksMsg::Timeout(req_id) => state.timeout(req_id, eff).await,
    }
    state
}

fn close_fetch_block_span(hash: HeaderHash) {
    {
        let Ok(mut spans) = FETCH_BLOCK_SPANS.lock() else {
            return;
        };
        spans.remove(&hash);
    }
}

fn close_fetch_block_spans(hashes: impl IntoIterator<Item = HeaderHash>) {
    if let Ok(mut spans) = FETCH_BLOCK_SPANS.lock() {
        for hash in hashes {
            spans.remove(&hash);
        }
    }
}

fn recover_stored_blocks_context(tip: Tip) -> TraceContext {
    let span = amaru_observability::trace_span!(
        amaru_observability::amaru::consensus::RECOVER_STORED_BLOCKS,
        hash = tip.hash(),
        point = tip.point().to_string(),
        slot = tip.slot().as_u64(),
        block_height = tip.block_height().as_u64()
    );
    TraceContext::from_span(&span)
}

fn recovered_process_block_context(parent_context: &TraceContext, tip: Tip) -> TraceContext {
    let span = amaru_observability::trace_span!(
        parent_context: parent_context,
        amaru_observability::amaru::consensus::PROCESS_BLOCK,
        hash = tip.hash(),
        point = tip.point().to_string(),
        slot = tip.slot().as_u64(),
        block_height = tip.block_height().as_u64()
    );
    TraceContext::from_span(&span)
}

/// Ensure that straggling block replies do not clog the mailbox of the fetch stage.
pub async fn cleanup_replies(
    (curr_id, fetch): (u64, StageRef<FetchBlocksMsg>),
    msg: Blocks2,
    eff: Effects<Blocks2>,
) -> (u64, StageRef<FetchBlocksMsg>) {
    match msg {
        // completely ignore empty responses, fetch stage will deal with timeouts
        Blocks2::NoBlocks(_) => (curr_id, fetch),
        // ignore responses to prior requests
        Blocks2::Block(id, _) if id < curr_id => (curr_id, fetch),
        Blocks2::Block(id, block) => {
            eff.send(&fetch, FetchBlocksMsg::Block(block)).await;
            // getting higher id implies a new request has started
            (id.max(curr_id), fetch)
        }
        // getting done message implies a new request will start with id+1, but Done might be old as well
        Blocks2::Done(id) => ((id + 1).max(curr_id), fetch),
    }
}

#[cfg(test)]
mod test_setup;
#[cfg(test)]
mod tests;
