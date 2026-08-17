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

// TODO: duplicate module with stages::adopt_chain::test_setup?

use std::sync::Arc;

use amaru_kernel::{EraHistory, Header, HeaderHash, IsHeader, Point, make_header, make_header_with_op_cert_seq};
use amaru_ouroboros_traits::{
    BaseReadChainStore, MockBlockValidator, WriteChainStore, has_stake_pools::MockHasStakePools,
    in_memory_chain_store::InMemoryChainStore,
};
use amaru_protocols::store_effects::{
    GetAnchorHashEffect, IsOnBestChainEffect, LoadBlockEffect, LoadHeaderEffect, LoadHeaderWithValidityEffect,
    ResourceHeaderStore,
};
use amaru_pure_stage::{
    DeserializerGuards, Effect, Name, StageGraph, StageRef, TerminationReason, simulation::SimulationRunning,
    trace_buffer::TraceEntry,
};
use tokio::runtime::{Builder, Runtime};

use super::*;
pub use crate::stages::test_utils::assert_trace;
use crate::{
    effects::{
        RecordMetricsEffect, ResourceBlockValidation, ResourceHasStakePools, SwitchToForkEffect, TipEffect,
        ValidateBlockEffect,
    },
    stages::{
        block_source::BlockSourceMsg,
        test_utils::{Logs, TraceMatch, run_simulation, tm_external_effect},
    },
};

/// Header tree for testing validate_block2 control flow.
/// Structure matches select_chain_new + adopt_chain for consistency:
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
        let h0 = make_header(1, 1, None);
        let h1 = make_header(2, 2, Some(h0.hash()));
        let h2 = make_header_with_op_cert_seq(3, 3, Some(h1.hash()), 1);
        let h3 = make_header_with_op_cert_seq(4, 4, Some(h2.hash()), 1);
        let h2a = make_header(3, 10, Some(h1.hash()));
        let h3a = make_header(4, 11, Some(h2a.hash()));
        Self { h0, h1, h2, h3, h2a, h3a }
    }

    pub fn main(&self) -> [&Header; 4] {
        [&self.h0, &self.h1, &self.h2, &self.h3]
    }

    pub fn all(&self) -> [&Header; 6] {
        [&self.h0, &self.h1, &self.h2, &self.h3, &self.h2a, &self.h3a]
    }
}

impl Default for HeaderTree {
    fn default() -> Self {
        Self::new()
    }
}

/// Bundles state, runtime, store, ledger, and refs for validate_block2 tests.
pub struct TestPrep {
    pub state: ValidateBlock,
    pub rt: Runtime,
    pub headers: HeaderTree,
    pub store: Arc<InMemoryChainStore>,
    pub block_validator: Arc<MockBlockValidator>,
}

impl TestPrep {
    pub fn set_current(&mut self, current: Point) {
        self.state.current = current;
        self.block_validator.with_tip(current);
    }

    pub fn store_headers(&self, headers: &[&Header]) {
        for h in headers {
            self.store.store_header(h).unwrap();
        }
    }

    pub fn store_blocks(&self, headers: &[&Header]) {
        for h in headers {
            let raw = amaru_kernel::cardano::network_block::make_encoded_block(h, &EraHistory::default());
            self.store.store_block(&h.hash(), &raw).unwrap();
        }
    }

    pub fn store_block(&self, header: &Header) {
        self.store_blocks(&[header]);
    }

    pub fn set_anchor(&self, hash: HeaderHash) {
        self.store.set_anchor_point(&self.store.load_point(&hash).unwrap_or(Point::Origin)).unwrap();
    }

    pub fn set_validity(&self, hash: HeaderHash, valid: bool) {
        self.store.set_block_valid(&hash, valid).unwrap();
    }

    pub fn roll_forward_chain(&self, point: Point) {
        self.store.roll_forward_chain(&point).unwrap();
    }
}

pub fn register_guards() -> DeserializerGuards {
    vec![
        amaru_pure_stage::register_data_deserializer::<ValidateBlock>().boxed(),
        amaru_pure_stage::register_data_deserializer::<ValidateBlockMsg>().boxed(),
        amaru_pure_stage::register_data_deserializer::<SelectChainMsg>().boxed(),
        amaru_pure_stage::register_data_deserializer::<AdoptChainMsg>().boxed(),
        amaru_pure_stage::register_data_deserializer::<BlockSourceMsg>().boxed(),
        amaru_pure_stage::register_data_deserializer::<Point>().boxed(),
        amaru_pure_stage::register_data_deserializer::<amaru_kernel::cardano::network_block::NetworkBlock>().boxed(),
        amaru_pure_stage::register_data_deserializer::<Option<(Header, Option<bool>)>>().boxed(),
        amaru_pure_stage::register_data_deserializer::<Option<HeaderHash>>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<LoadHeaderEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<LoadBlockEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<LoadHeaderWithValidityEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<IsOnBestChainEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<GetAnchorHashEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<TipEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<SwitchToForkEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<ValidateBlockEffect>().boxed(),
        amaru_pure_stage::register_effect_deserializer::<RecordMetricsEffect>().boxed(),
    ]
}

pub fn test_prep() -> TestPrep {
    let manager = StageRef::named_for_tests("manager");
    let select_chain = StageRef::named_for_tests("select_chain");
    let block_source = StageRef::named_for_tests("block_source");
    let block_validator = Arc::new(MockBlockValidator::new(Point::Origin));

    let state = ValidateBlock::new(manager.clone(), select_chain.clone(), block_source.clone(), 10, Point::Origin);

    TestPrep {
        state,
        rt: Builder::new_current_thread().build().unwrap(),
        headers: HeaderTree::new(),
        store: Arc::new(InMemoryChainStore::new()),
        block_validator,
    }
}

pub fn setup(prep: &TestPrep, msg: ValidateBlockMsg) -> (SimulationRunning, DeserializerGuards, Logs) {
    setup_many(prep, vec![msg])
}

pub fn setup_many(prep: &TestPrep, msgs: Vec<ValidateBlockMsg>) -> (SimulationRunning, DeserializerGuards, Logs) {
    let guards = register_guards();

    run_simulation(
        prep.rt.handle(),
        guards,
        |mut network| {
            let vb = network.stage("vb", stage);
            let vb = network.wire_up(vb, prep.state.clone());
            network.preload(&vb, msgs).unwrap();
            network
        },
        |resources| {
            resources.put::<ResourceHeaderStore>(prep.store.clone());
            resources.put::<ResourceBlockValidation>(prep.block_validator.clone());
            resources.put::<ResourceHasStakePools>(Arc::new(MockHasStakePools));
        },
        |_running| {
            // No special external effect overrides needed for most validate_block tests.
            // Virtual child stages are enabled by default.
        },
    )
}

pub fn te_validate_block(at_stage: &str, point: Point) -> TraceEntry {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(ValidateBlockEffect::new(&point))))
}

pub fn te_rollback_ledger(at_stage: &str, tip: &Point) -> TraceEntry {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(SwitchToForkEffect::new(tip))))
}

pub fn te_send(from: impl AsRef<str>, to: impl AsRef<str>, msg: impl amaru_pure_stage::SendData) -> TraceEntry {
    TraceEntry::suspend(amaru_pure_stage::Effect::send(from, to, Box::new(msg)))
}

pub fn te_terminate(at_stage: impl AsRef<str>) -> TraceEntry {
    TraceEntry::suspend(Effect::Terminate { at_stage: Name::from(at_stage.as_ref()) })
}

pub fn te_terminated(at_stage: impl AsRef<str>, reason: TerminationReason) -> TraceEntry {
    TraceEntry::Terminated { stage: Name::from(at_stage.as_ref()), reason }
}

/// Returns a TraceMatch that matches any RecordMetricsEffect for the given stage.
/// Use this (instead of a te_record_metrics literal) in assert_trace_contains / assert_trace_match
/// lists because the exact metrics payload can vary.
pub fn tm_record_metrics(at_stage: &str) -> TraceMatch<'static> {
    tm_external_effect::<RecordMetricsEffect>(at_stage)
}
