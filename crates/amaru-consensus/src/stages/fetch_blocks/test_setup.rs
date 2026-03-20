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

use std::sync::Arc;

use amaru_kernel::{
    BlockHeader, HeaderHash, Point, RawBlock, TESTNET_ERA_HISTORY, Tip,
    cardano::network_block::{make_block_with_header, make_encoded_block, make_network_block},
};
use amaru_ouroboros_traits::{ChainStore, in_memory_consensus_store::InMemConsensusStore};
use amaru_protocols::store_effects::{
    GetAnchorHashEffect, LoadBlockEffect, LoadHeaderEffect, ResourceHeaderStore, StoreBlockEffect,
};
use pure_stage::{
    DeserializerGuards, Effect, Instant, Name, ScheduleId, ScheduleIds, StageGraph, StageRef, TerminationReason,
    simulation::{SimulationBuilder, SimulationRunning},
    trace_buffer::{TraceBuffer, TraceEntry},
};
use tokio::runtime::{Builder, Runtime};
use tracing::Level;
use tracing_subscriber::util::SubscriberInitExt;

use super::*;
use crate::stages::{
    select_chain_new::SelectChainMsg,
    test_utils::{BufferWriter, Logs},
};

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
    pub cleanup_replies: StageRef<Blocks2>,
    pub headers: HeaderChain,
    pub store: Arc<InMemConsensusStore<BlockHeader>>,
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
        make_encoded_block(header, &TESTNET_ERA_HISTORY)
    }

    pub fn network_block(header: &BlockHeader) -> NetworkBlock {
        make_network_block(header, &TESTNET_ERA_HISTORY)
    }

    pub fn set_anchor(&self, hash: HeaderHash) {
        self.store.set_anchor_hash(&hash).unwrap();
    }

    pub fn schedule_at(&self, duration: Duration) -> ScheduleId {
        ScheduleIds::default().next_at(Instant::at_offset(duration))
    }

    pub fn state_with_request(&self, missing: Vec<Point>, req_id: u64, timeout: ScheduleId) -> FetchBlocks {
        FetchBlocks { req_id, missing, timeout: Some(timeout), ..self.state.clone() }
    }
}

pub fn register_guards() -> DeserializerGuards {
    vec![
        pure_stage::register_data_deserializer::<FetchBlocks>().boxed(),
        pure_stage::register_data_deserializer::<FetchBlocksMsg>().boxed(),
        pure_stage::register_data_deserializer::<SelectChainMsg>().boxed(),
        pure_stage::register_data_deserializer::<ManagerMessage>().boxed(),
        pure_stage::register_data_deserializer::<amaru_kernel::cardano::network_block::NetworkBlock>().boxed(),
        pure_stage::register_data_deserializer::<(Tip, Point)>().boxed(),
        pure_stage::register_effect_deserializer::<LoadHeaderEffect>().boxed(),
        pure_stage::register_effect_deserializer::<LoadBlockEffect>().boxed(),
        pure_stage::register_effect_deserializer::<GetAnchorHashEffect>().boxed(),
        pure_stage::register_effect_deserializer::<StoreBlockEffect>().boxed(),
    ]
}

/// Creates test prep with mocked cleanup_replies (named dummy StageRef).
pub fn test_prep() -> TestPrep {
    let downstream = StageRef::named_for_tests("downstream");
    let upstream = StageRef::named_for_tests("upstream");
    let manager = StageRef::named_for_tests("manager");
    let cleanup_replies = StageRef::named_for_tests("cleanup_replies");

    let state = FetchBlocks::for_tests(downstream, upstream, manager, cleanup_replies.clone());

    TestPrep {
        state,
        rt: Builder::new_current_thread().build().unwrap(),
        cleanup_replies,
        headers: HeaderChain::new(),
        store: Arc::new(InMemConsensusStore::new()),
    }
}

pub fn setup(prep: &TestPrep, msg: FetchBlocksMsg) -> (SimulationRunning, DeserializerGuards, Logs) {
    let writer = BufferWriter::new();
    let mut logs = writer.clone();

    let sub = tracing_subscriber::fmt()
        .with_max_level(Level::DEBUG)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .set_default();
    logs.set_guard(sub);

    let guards = register_guards();

    let mut network = SimulationBuilder::default().with_trace_buffer(TraceBuffer::new_shared(100, 1000000));
    network.resources().put::<ResourceHeaderStore>(prep.store.clone());

    let fb = network.stage("fb", stage);
    let fb = network.wire_up(fb, prep.state.clone());
    network.preload(&fb, [msg]).unwrap();

    let mut running = network.run();
    running.run_until_blocked_incl_effects(prep.rt.handle());

    (running, guards, logs.logs())
}

pub fn te_load_header(at_stage: &str, hash: HeaderHash) -> TraceEntry {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(LoadHeaderEffect::new(hash))))
}

pub fn te_load_block(at_stage: &str, hash: HeaderHash) -> TraceEntry {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(LoadBlockEffect::new(hash))))
}

pub fn te_get_anchor_hash(at_stage: &str) -> TraceEntry {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(GetAnchorHashEffect::new())))
}

pub fn te_store_block(at_stage: &str, hash: HeaderHash, block: amaru_kernel::RawBlock) -> TraceEntry {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(StoreBlockEffect::new(&hash, block))))
}

pub fn te_send(from: impl AsRef<str>, to: impl AsRef<str>, msg: impl pure_stage::SendData) -> TraceEntry {
    TraceEntry::suspend(pure_stage::Effect::send(from, to, Box::new(msg)))
}

pub fn te_terminate(at_stage: impl AsRef<str>) -> TraceEntry {
    TraceEntry::suspend(Effect::Terminate { at_stage: Name::from(at_stage.as_ref()) })
}

pub fn te_schedule(at_stage: impl AsRef<str>, msg: impl pure_stage::SendData, schedule_id: ScheduleId) -> TraceEntry {
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

pub fn te_terminated(at_stage: impl AsRef<str>, reason: TerminationReason) -> TraceEntry {
    TraceEntry::Terminated { stage: Name::from(at_stage.as_ref()), reason }
}

#[track_caller]
pub fn assert_trace(running: &SimulationRunning, expected: &[TraceEntry]) {
    let mut tb = running.trace_buffer().lock();
    let trace = tb
        .iter_entries()
        .filter_map(|(_, e)| (!matches!(e, TraceEntry::Resume { .. })).then_some(e))
        .collect::<Vec<_>>();
    tb.clear();
    pretty_assertions::assert_eq!(trace, expected);
}
