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

use std::{sync::Arc, time::Duration};

use amaru_kernel::{
    BlockHeader, EraHistory, HeaderHash, Peer, RawBlock,
    cardano::network_block::{make_block_with_header, make_encoded_block, make_network_block},
};
use amaru_ouroboros_traits::{MissingBlocks, StoreError, WriteChainStore, in_memory_chain_store::InMemoryChainStore};
use amaru_protocols::store_effects::{
    AncestorsBetweenEffect, FindMissingBlocksEffect, GetAnchorHashEffect, GetChildrenEffect, HasBlockEffect,
    LoadHeaderEffect, LoadHeaderWithValidityEffect, LoadTipEffect, ResourceHeaderStore, StoreBlockEffect,
    UnvalidatedAncestorHashesEffect,
};
use amaru_pure_stage::{
    DeserializerGuards, Effect, Instant, Name, ScheduleId, ScheduleIds, StageGraph, StageRef,
    simulation::SimulationRunning, trace_buffer::TraceEntry,
};
use tokio::runtime::{Builder, Runtime};

use super::*;
use crate::stages::{
    block_source::BlockSourceMsg,
    select_chain::SelectChainMsg,
    test_utils::{Logs, run_simulation},
};

pub fn test_peer() -> Peer {
    Peer::new("test-peer")
}

pub fn make_block_header(block_number: u64, slot: u64, parent: Option<HeaderHash>) -> BlockHeader {
    let header = amaru_kernel::make_header(block_number, slot, parent);
    let block = make_block_with_header(&header.into());
    block.header.into()
}

/// Simple header chain for fetch_blocks tests: h0 (genesis) -> h1 -> h2.
#[derive(Clone)]
pub struct HeaderChain {
    pub h0: BlockHeader,
    pub h1: BlockHeader,
    pub h2: BlockHeader,
}

impl HeaderChain {
    pub fn new() -> Self {
        let h0 = make_block_header(1, 1, None);
        let h1 = make_block_header(2, 2, Some(h0.hash()));
        let h2 = make_block_header(3, 3, Some(h1.hash()));
        Self { h0, h1, h2 }
    }
}

impl Default for HeaderChain {
    fn default() -> Self {
        Self::new()
    }
}

/// Bundles state, runtime, store, and refs for fetch_blocks tests.
pub struct TestPrep {
    pub state: FetchBlocks,
    pub rt: Runtime,
    pub cleanup_replies: StageRef<Blocks>,
    pub headers: HeaderChain,
    pub store: Arc<InMemoryChainStore>,
}

impl TestPrep {
    pub fn store_headers(&self, headers: &[&BlockHeader]) {
        for h in headers {
            self.store.store_header(h).unwrap();
        }
    }

    pub fn store_block(&self, header: &BlockHeader) {
        let raw = Self::raw_block(header);
        self.store.store_block(&header.hash(), &raw).unwrap();
    }

    pub fn raw_block(header: &BlockHeader) -> RawBlock {
        make_encoded_block(header, &EraHistory::default())
    }

    pub fn network_block(header: &BlockHeader) -> NetworkBlock {
        make_network_block(header, &EraHistory::default())
    }

    pub fn set_anchor(&self, hash: HeaderHash) {
        self.store.set_anchor_hash(&hash).unwrap();
    }

    pub fn set_validity(&self, hash: HeaderHash, valid: bool) {
        self.store.set_block_valid(&hash, valid).unwrap();
    }

    pub fn schedule_at(&self, duration: Duration) -> ScheduleId {
        ScheduleIds::default().next_at(Instant::at_offset(duration, Duration::ZERO))
    }

    pub fn state_with_request(&self, missing: MissingBlocks, req_id: u64, timeout: ScheduleId) -> FetchBlocks {
        FetchBlocks { req_id, missing: Some(missing), timeout: Some(timeout), ..self.state.clone() }
    }

    pub fn state_with_block_height(&self, block_height: u64) -> FetchBlocks {
        FetchBlocks { block_height: BlockHeight::from(block_height), ..self.state.clone() }
    }
}

pub fn register_guards() -> DeserializerGuards {
    vec![
        amaru_pure_stage::register_data_deserializer::<FetchBlocks>().boxed(),
        amaru_pure_stage::register_data_deserializer::<Cleanup>().boxed(),
        amaru_pure_stage::register_data_deserializer::<FetchBlocksMsg>().boxed(),
        amaru_pure_stage::register_data_deserializer::<SelectChainMsg>().boxed(),
        amaru_pure_stage::register_data_deserializer::<ManagerMessage>().boxed(),
        amaru_pure_stage::register_data_deserializer::<amaru_kernel::Peer>().boxed(),
        amaru_pure_stage::register_data_deserializer::<BlockSourceMsg>().boxed(),
        amaru_pure_stage::register_data_deserializer::<amaru_kernel::cardano::network_block::NetworkBlock>().boxed(),
        amaru_pure_stage::register_data_deserializer::<DownloadedBlock>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<LoadHeaderEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<LoadHeaderWithValidityEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<HasBlockEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<GetAnchorHashEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<GetChildrenEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<LoadTipEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<StoreBlockEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<FindMissingBlocksEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<UnvalidatedAncestorHashesEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<AncestorsBetweenEffect>().boxed(),
        amaru_pure_stage::register_data_deserializer::<(Vec<HeaderHash>, bool)>().boxed(),
        amaru_pure_stage::register_data_deserializer::<Option<Vec<amaru_kernel::Tip>>>().boxed(),
        amaru_pure_stage::register_data_deserializer::<Result<Option<MissingBlocks>, StoreError>>().boxed(),
    ]
}

/// Creates test prep with mocked cleanup_replies (named dummy StageRef).
pub fn test_prep() -> TestPrep {
    let downstream = StageRef::named_for_tests("downstream");
    let upstream = StageRef::named_for_tests("upstream");
    let manager = StageRef::named_for_tests("manager");
    let block_source = StageRef::named_for_tests("block_source");
    let peer_selection = StageRef::named_for_tests("peer_selection");
    let cleanup_replies = StageRef::named_for_tests("cleanup_replies");

    let state =
        FetchBlocks::for_tests(downstream, upstream, manager, block_source, peer_selection, cleanup_replies.clone());

    TestPrep {
        state,
        rt: Builder::new_current_thread().build().unwrap(),
        cleanup_replies,
        headers: HeaderChain::new(),
        store: Arc::new(InMemoryChainStore::new()),
    }
}

pub fn setup(prep: &TestPrep, msg: FetchBlocksMsg) -> (SimulationRunning, DeserializerGuards, Logs) {
    let guards = register_guards();

    run_simulation(
        prep.rt.handle(),
        guards,
        |mut network| {
            let fb = network.stage("fb", stage);
            let fb = network.wire_up(fb, prep.state.clone());
            network.preload(&fb, [msg]).unwrap();
            network
        },
        |resources| {
            resources.put::<ResourceHeaderStore>(prep.store.clone());
        },
        |_running| {
            // No additional external effect overrides needed for basic fetch_blocks tests.
            // Virtual child stages are enabled by default in run_simulation.
        },
    )
}

pub fn te_find_missing_blocks(at_stage: &str, start: HeaderHash, limit: usize) -> TraceEntry {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(FindMissingBlocksEffect::new(start, limit))))
}

pub fn te_has_block(at_stage: &str, hash: HeaderHash) -> TraceEntry {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(HasBlockEffect::new(hash))))
}

pub fn te_ancestors_between(at_stage: &str, from: amaru_kernel::Point, to: HeaderHash) -> TraceEntry {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(AncestorsBetweenEffect::new(from, to))))
}

pub fn te_load_header(at_stage: &str, hash: HeaderHash, with_validity: bool) -> TraceEntry {
    TraceEntry::suspend(Effect::external(
        at_stage,
        if with_validity {
            Box::new(LoadHeaderWithValidityEffect::new(hash))
        } else {
            Box::new(LoadHeaderEffect::new(hash))
        },
    ))
}

pub fn te_store_block(at_stage: &str, hash: HeaderHash, block: amaru_kernel::RawBlock) -> TraceEntry {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(StoreBlockEffect::new(&hash, block))))
}

pub fn te_schedule(
    at_stage: impl AsRef<str>,
    msg: impl amaru_pure_stage::SendData,
    schedule_id: ScheduleId,
) -> TraceEntry {
    TraceEntry::suspend(Effect::Schedule {
        at_stage: Name::from(at_stage.as_ref()),
        msg: Box::new(msg),
        id: schedule_id,
    })
}

pub fn te_cancel_schedule(at_stage: impl AsRef<str>, schedule_id: ScheduleId) -> TraceEntry {
    TraceEntry::suspend(Effect::CancelSchedule { at_stage: Name::from(at_stage.as_ref()), id: schedule_id })
}

pub fn te_clock(instant: Instant) -> TraceEntry {
    TraceEntry::Clock(instant)
}
