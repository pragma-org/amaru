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
    EraHistory, Header, HeaderHash, IsHeader, Point, cardano::network_block::EncodedTestBlock, make_header,
    make_header_with_op_cert_seq,
};
use amaru_ouroboros_traits::{BaseReadChainStore, ChainStore, in_memory_chain_store::InMemoryChainStore};
use amaru_protocols::store_effects::{
    GetAnchorHashEffect, GetBestChainHashEffect, GetChildrenEffect, HasHeaderEffect, LoadHeaderEffect,
    LoadHeaderWithValidityEffect, LoadPointEffect, ResourceHeaderStore, SetBlockValidEffect,
    UnvalidatedAncestorHashesEffect,
};
use amaru_pure_stage::{
    DeserializerGuards, Effect, StageGraph, StageRef, simulation::SimulationRunning, trace_buffer::TraceEntry,
};
use tokio::runtime::Runtime;

use super::*;
use crate::stages::test_utils::{Logs, run_simulation};

pub fn make_block_header(block_number: u64, slot: u64, parent: Option<HeaderHash>) -> Header {
    EncodedTestBlock::from_seed(&make_header(block_number, slot, parent), &EraHistory::default()).header
}

pub fn make_block_header_with_op_cert_seq(
    block_number: u64,
    slot: u64,
    parent: Option<HeaderHash>,
    op_cert_seq: u64,
) -> Header {
    EncodedTestBlock::from_seed(
        &make_header_with_op_cert_seq(block_number, slot, parent, op_cert_seq),
        &EraHistory::default(),
    )
    .header
}

/// Header tree for testing block invalidation and chain selection:
/// - h0: genesis (block 1, slot 1, no parent)
///   - h1: block 2, slot 2, parent h0
///     - h2: block 3, slot 3, parent h1
///       - h3: block 4, slot 4, parent h2   (main chain tip)
///     - h2a: block 3, slot 10, parent h1 (fork at h1)
///       - h3a: block 4, slot 11, parent h2a (fork tip)
#[derive(Clone)]
pub struct HeaderTree {
    pub h0: Header,
    pub h1: Header,
    pub h2: Header,
    pub h3: Header,
    pub h2a: Header,
    pub h3a: Header,
}

impl HeaderTree {
    pub fn new() -> Self {
        let h0 = make_block_header(1, 1, None);
        let h1 = make_block_header(2, 2, Some(h0.hash()));
        let h2 = make_block_header_with_op_cert_seq(3, 3, Some(h1.hash()), 1);
        let h3 = make_block_header_with_op_cert_seq(4, 4, Some(h2.hash()), 1);
        let h2a = make_block_header(3, 10, Some(h1.hash()));
        let h3a = make_block_header(4, 11, Some(h2a.hash()));
        Self { h0, h1, h2, h3, h2a, h3a }
    }

    pub fn main(&self) -> [&Header; 4] {
        [&self.h0, &self.h1, &self.h2, &self.h3]
    }

    pub fn all(&self) -> [&Header; 6] {
        [&self.h0, &self.h1, &self.h2, &self.h3, &self.h2a, &self.h3a]
    }
}

/// Bundles state, runtime, downstream ref, and header tree for tests.
pub struct TestPrep {
    pub state: SelectChain,
    pub rt: Runtime,
    pub downstream: StageRef<NewBestTip>,
    pub headers: HeaderTree,
    pub store: Arc<dyn ChainStore>,
}

impl TestPrep {
    pub fn store_headers(&self, headers: &[&Header]) {
        for h in headers {
            self.store.store_header(h).unwrap();
        }
    }

    pub fn set_validity(&self, hash: HeaderHash, valid: bool) {
        self.store.set_block_valid(&hash, valid).unwrap();
    }

    pub fn set_anchor(&self, hash: HeaderHash) {
        self.store.set_anchor_point(&self.store.load_point(&hash).unwrap_or(Point::Origin)).unwrap();
    }

    pub fn set_best_chain(&self, hash: HeaderHash) {
        self.store.set_best_chain_tip(&self.header(hash).point()).unwrap();
    }

    pub fn header(&self, hash: HeaderHash) -> Header {
        self.headers.all().iter().find(|h| h.hash() == hash).copied().unwrap().clone()
    }
}

pub fn register_guards() -> DeserializerGuards {
    vec![
        amaru_pure_stage::register_data_deserializer::<SelectChain>().boxed(),
        amaru_pure_stage::register_data_deserializer::<SelectChainMsg>().boxed(),
        amaru_pure_stage::register_data_deserializer::<Point>().boxed(),
        amaru_pure_stage::register_data_deserializer::<(Point, Point)>().boxed(),
        amaru_pure_stage::register_data_deserializer::<NewBestTip>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<LoadHeaderEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<GetAnchorHashEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<GetBestChainHashEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<GetChildrenEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<LoadHeaderWithValidityEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<LoadPointEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<SetBlockValidEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<HasHeaderEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<UnvalidatedAncestorHashesEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<FindBestCandidate>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::RecordHeaderAbandonedEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::RecordBlockValidEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::RecordBlockPrunedEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<crate::performance::RecordForkStartedEffect>().boxed(),
        amaru_pure_stage::register_data_deserializer::<(Vec<HeaderHash>, bool)>().boxed(),
    ]
}

/// Creates test prep with Point::Origin as best_tip and empty tips (just origin).
pub fn test_prep() -> TestPrep {
    let downstream = StageRef::named_for_tests("downstream");
    let mut state = SelectChain::new(downstream.clone());
    state.may_fetch_blocks = true;
    TestPrep {
        state,
        rt: crate::stages::test_utils::test_runtime(),
        downstream,
        headers: HeaderTree::new(),
        store: Arc::new(InMemoryChainStore::new()),
    }
}

pub fn setup(prep: &TestPrep, msg: SelectChainMsg) -> (SimulationRunning, DeserializerGuards, Logs) {
    setup_many(prep, vec![msg])
}

pub fn setup_many(prep: &TestPrep, msgs: Vec<SelectChainMsg>) -> (SimulationRunning, DeserializerGuards, Logs) {
    run_simulation(
        prep.rt.handle(),
        register_guards(),
        |mut network| {
            let sc = network.stage("sc", stage);
            let sc = network.wire_up(sc, prep.state.clone());
            network.preload(&sc, msgs).unwrap();
            network
        },
        |resources| {
            resources.put::<crate::performance::ResourcePerformance>(std::sync::Arc::new(
                crate::performance::Performance::new(),
            ));
            resources.put::<ResourceHeaderStore>(prep.store.clone());
        },
        |_running| {
            // No external effect overrides needed for most select_chain tests.
            // Virtual child stages are enabled by default in run_simulation.
        },
    )
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

pub fn te_has_header(at_stage: &str, hash: HeaderHash) -> TraceEntry {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(HasHeaderEffect::new(hash))))
}

pub fn te_find_best_candidate(at_stage: &str) -> TraceEntry {
    TraceEntry::Suspend(Effect::external(at_stage, Box::new(FindBestCandidate)))
}

pub fn te_load_point(at_stage: &str, hash: HeaderHash) -> TraceEntry {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(LoadPointEffect::new(hash))))
}

pub fn te_set_block_valid(at_stage: &str, hash: HeaderHash, valid: bool) -> TraceEntry {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(SetBlockValidEffect::new(hash, valid))))
}

pub fn te_unvalidated_ancestor_hashes(at_stage: &str, start: HeaderHash) -> TraceEntry {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(UnvalidatedAncestorHashesEffect::new(start))))
}

pub fn te_record_header_abandoned(at_stage: &str, hash: HeaderHash, now: amaru_pure_stage::Instant) -> TraceEntry {
    TraceEntry::suspend(Effect::external(
        at_stage,
        Box::new(crate::performance::Performance::record_header_abandoned(hash, now)),
    ))
}

pub fn te_record_block_valid(
    at_stage: &str,
    hash: HeaderHash,
    now: amaru_pure_stage::Instant,
    syncing: bool,
) -> TraceEntry {
    TraceEntry::suspend(Effect::external(
        at_stage,
        Box::new(crate::performance::Performance::record_block_valid(hash, now, syncing)),
    ))
}

pub fn te_record_block_pruned(
    at_stage: &str,
    hash: HeaderHash,
    invalid: bool,
    now: amaru_pure_stage::Instant,
    syncing: bool,
) -> TraceEntry {
    TraceEntry::suspend(Effect::external(
        at_stage,
        Box::new(crate::performance::Performance::record_block_pruned(hash, invalid, now, syncing)),
    ))
}

pub fn te_record_fork_started(at_stage: &str, tip: Point, now: amaru_pure_stage::Instant) -> TraceEntry {
    TraceEntry::suspend(Effect::external(
        at_stage,
        Box::new(crate::performance::Performance::record_fork_started(tip, now)),
    ))
}
