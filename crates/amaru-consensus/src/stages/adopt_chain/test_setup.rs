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
    BlockHeader, BlockHeight, HeaderHash, NonEmptyVec, Point, Tip, make_header, make_header_with_op_cert_seq,
};
use amaru_ouroboros::StoreError;
use amaru_ouroboros_traits::{ChainStore, in_memory_consensus_store::InMemConsensusStore};
use amaru_protocols::store_effects::{
    FindAncestorOnBestChainEffect, FindAnchorAtHeightEffect, GetAnchorHashEffect, GetBestChainHashEffect,
    LoadFromBestChainEffect, LoadHeaderEffect, NextBestChainEffect, ResourceHeaderStore, RollForwardChainEffect,
    SetAnchorHashEffect, SwitchToForkEffect,
};
use pure_stage::{
    DeserializerGuards, Effect, Name, StageGraph, StageRef, TerminationReason,
    simulation::{SimulationBuilder, SimulationRunning},
    trace_buffer::{TraceBuffer, TraceEntry},
};
use tokio::runtime::{Builder, Runtime};
use tracing::Level;
use tracing_subscriber::util::SubscriberInitExt;

use super::*;
use crate::stages::{
    block_source::BlockSourceMsg,
    test_utils::{BufferWriter, Logs},
};

pub fn make_block_header(block_number: u64, slot: u64, parent: Option<HeaderHash>) -> BlockHeader {
    BlockHeader::from(make_header(block_number, slot, parent))
}

pub fn make_block_header_with_op_cert_seq(
    block_number: u64,
    slot: u64,
    parent: Option<HeaderHash>,
    op_cert_seq: u64,
) -> BlockHeader {
    BlockHeader::from(make_header_with_op_cert_seq(block_number, slot, parent, op_cert_seq))
}

/// Header tree for testing adopt_chain control flow:
/// - h0: genesis (block 1, slot 1, no parent)
///   - h1: block 2, slot 2, parent h0
///     - h2: block 3, slot 3, parent h1
///       - h3: block 4, slot 4, parent h2   (main chain tip, op_cert_seq 1)
///       - h4: block 5, slot 5, parent h3    (extension for anchor drag test)
///     - h2a: block 3, slot 10, parent h1 (fork at h1)
///       - h3a: block 4, slot 11, parent h2a (fork tip, op_cert_seq 0 - loses to h3)
#[derive(Clone, Debug)]
pub struct HeaderTree {
    pub h0: BlockHeader,
    pub h1: BlockHeader,
    pub h2: BlockHeader,
    pub h3: BlockHeader,
    pub h4: BlockHeader,
    pub h2a: BlockHeader,
    pub h3a: BlockHeader,
}

#[allow(dead_code)]
impl HeaderTree {
    pub fn new() -> Self {
        let h0 = make_block_header(1, 1, None);
        let h1 = make_block_header(2, 2, Some(h0.hash()));
        let h2 = make_block_header_with_op_cert_seq(3, 3, Some(h1.hash()), 1);
        let h3 = make_block_header_with_op_cert_seq(4, 4, Some(h2.hash()), 1);
        let h4 = make_block_header(5, 5, Some(h3.hash()));
        let h2a = make_block_header(3, 10, Some(h1.hash()));
        let h3a = make_block_header(4, 11, Some(h2a.hash()));
        Self { h0, h1, h2, h3, h4, h2a, h3a }
    }

    pub fn main_chain(&self) -> [&BlockHeader; 5] {
        [&self.h0, &self.h1, &self.h2, &self.h3, &self.h4]
    }

    pub fn fork_chain(&self) -> [&BlockHeader; 3] {
        [&self.h0, &self.h1, &self.h2a]
    }

    pub fn all(&self) -> [&BlockHeader; 7] {
        [&self.h0, &self.h1, &self.h2, &self.h3, &self.h4, &self.h2a, &self.h3a]
    }
}

/// Bundles state, runtime, downstream ref, and header tree for tests.
pub struct TestPrep {
    pub state: AdoptChain,
    pub rt: Runtime,
    pub headers: HeaderTree,
    pub store: Arc<dyn ChainStore<BlockHeader>>,
}

impl TestPrep {
    pub fn store_headers(&self, headers: &[&BlockHeader]) {
        for h in headers {
            self.store.store_header(h).unwrap();
        }
    }

    pub fn set_anchor(&self, hash: HeaderHash) {
        self.store.set_anchor_hash(&hash).unwrap();
    }

    pub fn set_best_chain(&mut self, header: BlockHeader) {
        self.state.current_best_tip = header.tip();
        let mut ancestors = self.store.ancestors(header).collect::<Vec<_>>();
        ancestors.reverse();
        for header in ancestors {
            self.store.roll_forward_chain(&header.point()).unwrap();
        }
    }
}

pub fn register_guards() -> DeserializerGuards {
    vec![
        pure_stage::register_data_deserializer::<AdoptChain>().boxed(),
        pure_stage::register_data_deserializer::<Tip>().boxed(),
        pure_stage::register_data_deserializer::<ManagerMessage>().boxed(),
        pure_stage::register_data_deserializer::<AdoptChainMsg>().boxed(),
        pure_stage::register_data_deserializer::<BlockSourceMsg>().boxed(),
        pure_stage::register_data_deserializer::<Option<BlockHeader>>().boxed(),
        pure_stage::register_data_deserializer::<Option<Point>>().boxed(),
        pure_stage::register_data_deserializer::<Result<(), StoreError>>().boxed(),
        pure_stage::register_effect_deserializer::<LoadHeaderEffect>().boxed(),
        pure_stage::register_effect_deserializer::<GetAnchorHashEffect>().boxed(),
        pure_stage::register_effect_deserializer::<GetBestChainHashEffect>().boxed(),
        pure_stage::register_effect_deserializer::<SwitchToForkEffect>().boxed(),
        pure_stage::register_effect_deserializer::<RollForwardChainEffect>().boxed(),
        pure_stage::register_effect_deserializer::<SetAnchorHashEffect>().boxed(),
        pure_stage::register_effect_deserializer::<LoadFromBestChainEffect>().boxed(),
        pure_stage::register_effect_deserializer::<NextBestChainEffect>().boxed(),
        pure_stage::register_effect_deserializer::<FindAncestorOnBestChainEffect>().boxed(),
        pure_stage::register_effect_deserializer::<FindAnchorAtHeightEffect>().boxed(),
        pure_stage::register_data_deserializer::<Option<(Point, NonEmptyVec<Point>)>>().boxed(),
        pure_stage::register_data_deserializer::<Option<HeaderHash>>().boxed(),
    ]
}

pub fn test_prep(consensus_security_param: u64) -> TestPrep {
    let downstream = StageRef::named_for_tests("downstream");
    let block_source = StageRef::named_for_tests("block_source");
    let headers = HeaderTree::new();
    let state = AdoptChain::new(downstream, block_source, consensus_security_param, Tip::origin());
    TestPrep {
        state,
        rt: Builder::new_current_thread().build().unwrap(),
        headers,
        store: Arc::new(InMemConsensusStore::new()),
    }
}

pub fn setup(prep: &TestPrep, msg: Tip) -> (SimulationRunning, DeserializerGuards, Logs) {
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

    let ac = network.stage("ac", stage);
    let ac = network.wire_up(ac, prep.state.clone());
    network.preload(&ac, [AdoptChainMsg::new(msg, BlockHeight::new(0))]).unwrap();

    let mut running = network.run();
    running.run_until_blocked_incl_effects(prep.rt.handle());

    (running, guards, logs.logs())
}

pub fn te_load_header(at_stage: &str, hash: HeaderHash) -> TraceEntry {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(LoadHeaderEffect::new(hash))))
}

pub fn te_set_anchor_hash(at_stage: &str, hash: HeaderHash) -> TraceEntry {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(SetAnchorHashEffect::new(hash))))
}

pub fn te_switch_to_fork(at_stage: &str, fork_point: Point, forward_points: NonEmptyVec<Point>) -> TraceEntry {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(SwitchToForkEffect::new(fork_point, forward_points))))
}

pub fn te_roll_forward_chain(at_stage: &str, point: Point) -> TraceEntry {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(RollForwardChainEffect::new(point))))
}

pub fn te_find_ancestor_on_best_chain(at_stage: &str, hash: HeaderHash) -> TraceEntry {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(FindAncestorOnBestChainEffect::new(hash))))
}

pub fn te_find_anchor_at_height(at_stage: &str, target_height: BlockHeight) -> TraceEntry {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(FindAnchorAtHeightEffect::new(target_height))))
}

pub fn te_send(from: impl AsRef<str>, to: impl AsRef<str>, msg: impl pure_stage::SendData) -> TraceEntry {
    TraceEntry::suspend(pure_stage::Effect::send(from, to, Box::new(msg)))
}

pub fn te_clock(at_stage: &str) -> TraceEntry {
    TraceEntry::suspend(pure_stage::Effect::clock(at_stage))
}

pub fn te_terminate(at_stage: impl AsRef<str>) -> TraceEntry {
    TraceEntry::suspend(Effect::Terminate { at_stage: Name::from(at_stage.as_ref()) })
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
