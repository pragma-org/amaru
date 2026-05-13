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

use std::{collections::BTreeSet, fmt, net::SocketAddr, sync::Arc};

use amaru_kernel::{
    BlockHeader, BlockHeight, HeaderHash, IsHeader, Point, TESTNET_ERA_HISTORY, Tip, make_header,
    make_header_with_op_cert_seq,
};
use amaru_metrics::ledger::LedgerMetrics;
use amaru_ouroboros_traits::{
    BlockValidationError, CanValidateBlocks, ChainStore, HasStakePools, in_memory_consensus_store::InMemConsensusStore,
};
use amaru_protocols::store_effects::{
    GetAnchorHashEffect, LoadBlockEffect, LoadFromBestChainEffect, LoadHeaderEffect, LoadHeaderWithValidityEffect,
    ResourceHeaderStore,
};
use async_trait::async_trait;
use parking_lot::Mutex;
use pure_stage::{
    DeserializerGuards, Effect, Name, StageGraph, StageRef, TerminationReason,
    simulation::{SimulationBuilder, SimulationRunning},
    trace_buffer::{TraceBuffer, TraceEntry},
};
use tokio::runtime::{Builder, Runtime};
use tracing::Level;
use tracing_subscriber::util::SubscriberInitExt;

use super::*;
use crate::{
    effects::{
        ContainsPointEffect, RecordMetricsEffect, ResourceBlockValidation, ResourceHasStakePools, RollbackBlockEffect,
        TipEffect, ValidateBlockEffect,
    },
    stages::{
        block_source::BlockSourceMsg,
        test_utils::{BufferWriter, Logs},
    },
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

/// Header tree for testing validate_block2 control flow.
/// Structure matches select_chain_new + adopt_chain for consistency:
/// - h0: genesis (block 1, slot 1, no parent)
///   - h1: block 2, slot 2, parent h0
///     - h2: block 3, slot 3, parent h1
///       - h3: block 4, slot 4, parent h2   (main chain tip)
///     - h2a: block 3, slot 10, parent h1 (fork at h1)
///       - h3a: block 4, slot 11, parent h2a (fork tip)
#[derive(Clone)]
#[allow(dead_code)] // h2a, h3a reserved for fork tests
pub struct HeaderTree {
    pub h0: BlockHeader,
    pub h1: BlockHeader,
    pub h2: BlockHeader,
    pub h3: BlockHeader,
    pub h2a: BlockHeader,
    pub h3a: BlockHeader,
}

#[allow(dead_code)] // fork reserved for fork-specific tests
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

    pub fn main(&self) -> [&BlockHeader; 4] {
        [&self.h0, &self.h1, &self.h2, &self.h3]
    }

    pub fn fork(&self) -> [&BlockHeader; 3] {
        [&self.h0, &self.h1, &self.h2a]
    }

    pub fn all(&self) -> [&BlockHeader; 6] {
        [&self.h0, &self.h1, &self.h2, &self.h3, &self.h2a, &self.h3a]
    }
}

impl Default for HeaderTree {
    fn default() -> Self {
        Self::new()
    }
}

/// Configurable mock ledger for testing rollback/roll-forward paths.
pub struct MockBlockValidator {
    inner: Mutex<MockBlockValidatorInner>,
}

struct MockBlockValidatorInner {
    /// Points the ledger is considered to contain (validated).
    contains: BTreeSet<Point>,
    /// Current ledger tip.
    tip: Point,
    /// If set, rollback_block will return this error.
    rollback_fails: bool,
    /// If set, roll_forward_block will return Ok(Err(...)) for these points.
    validate_fails: BTreeSet<Point>,
    /// If set, roll_forward_block will return Err(...) for these points.
    ledger_fails: BTreeSet<Point>,
}

impl MockBlockValidator {
    pub fn new(tip: Point) -> Self {
        Self {
            inner: Mutex::new(MockBlockValidatorInner {
                contains: BTreeSet::default(),
                tip,
                rollback_fails: false,
                validate_fails: BTreeSet::default(),
                ledger_fails: BTreeSet::default(),
            }),
        }
    }

    pub fn with_contains(&self, point: Point) -> &Self {
        self.inner.lock().contains.insert(point);
        self
    }

    pub fn with_rollback_fails(&self, fails: bool) -> &Self {
        self.inner.lock().rollback_fails = fails;
        self
    }

    pub fn with_validate_fails(&self, point: Point) -> &Self {
        self.inner.lock().validate_fails.insert(point);
        self
    }

    pub fn with_ledger_fails(&self, point: Point) -> &Self {
        self.inner.lock().ledger_fails.insert(point);
        self
    }

    pub fn with_tip(&self, tip: Point) -> &Self {
        self.inner.lock().tip = tip;
        self
    }
}

#[async_trait]
impl CanValidateBlocks for MockBlockValidator {
    async fn roll_forward_block(
        &self,
        point: &Point,
        _block: amaru_kernel::Block,
    ) -> Result<Result<LedgerMetrics, BlockValidationError>, BlockValidationError> {
        let mut inner = self.inner.lock();
        if inner.ledger_fails.contains(point) {
            return Err(BlockValidationError::new(anyhow::anyhow!("mock ledger failed")));
        }
        if inner.validate_fails.contains(point) {
            return Ok(Err(BlockValidationError::new(anyhow::anyhow!("mock validation failed"))));
        }
        inner.contains.insert(*point);
        Ok(Ok(Default::default()))
    }

    fn rollback_block(&self, to: &Point) -> Result<(), BlockValidationError> {
        let mut inner = self.inner.lock();
        if inner.rollback_fails {
            return Err(BlockValidationError::new(anyhow::anyhow!("mock rollback failed")));
        }
        inner.tip = *to;
        inner.contains.retain(|p| p.slot_or_default() <= to.slot_or_default());
        Ok(())
    }

    fn contains_point(&self, point: &Point) -> bool {
        self.inner.lock().contains.contains(point)
    }

    fn tip(&self) -> Point {
        self.inner.lock().tip
    }

    fn volatile_tip(&self) -> Option<Tip> {
        let inner = self.inner.lock();
        inner.contains.last().map(|p| Tip::new(*p, BlockHeight::from(inner.contains.len() as u64) + 1))
    }
}

#[async_trait]
impl HasStakePools for MockBlockValidator {
    async fn registered_relay_socket_addrs(&self) -> Result<BTreeSet<SocketAddr>, BlockValidationError> {
        Ok(BTreeSet::new())
    }
}

/// Bundles state, runtime, store, ledger, and refs for validate_block2 tests.
pub struct TestPrep {
    pub state: ValidateBlock,
    pub rt: Runtime,
    pub manager: StageRef<AdoptChainMsg>,
    pub select_chain: StageRef<SelectChainMsg>,
    pub block_source: StageRef<BlockSourceMsg>,
    pub headers: HeaderTree,
    pub store: Arc<InMemConsensusStore<BlockHeader>>,
    pub block_validator: Arc<MockBlockValidator>,
}

#[allow(dead_code)] // set_validity, set_best_chain reserved for future tests
impl TestPrep {
    pub fn set_current(&mut self, current: Point) {
        self.state.current = current;
        self.block_validator.with_tip(current);
    }

    pub fn store_headers(&self, headers: &[&BlockHeader]) {
        for h in headers {
            self.store.store_header(h).unwrap();
        }
    }

    pub fn store_blocks(&self, headers: &[&BlockHeader]) {
        for h in headers {
            let raw = amaru_kernel::cardano::network_block::make_encoded_block(h, &TESTNET_ERA_HISTORY);
            self.store.store_block(&h.hash(), &raw).unwrap();
        }
    }

    pub fn store_block(&self, header: &BlockHeader) {
        self.store_blocks(&[header]);
    }

    pub fn set_anchor(&self, hash: HeaderHash) {
        self.store.set_anchor_hash(&hash).unwrap();
    }

    pub fn set_validity(&self, hash: HeaderHash, valid: bool) {
        self.store.set_block_valid(&hash, valid).unwrap();
    }

    pub fn set_best_chain(&self, hash: HeaderHash) {
        self.store.set_best_chain_hash(&hash).unwrap();
    }

    pub fn roll_forward_chain(&self, point: Point) {
        self.store.roll_forward_chain(&point).unwrap();
    }
}

pub fn register_guards() -> DeserializerGuards {
    vec![
        pure_stage::register_data_deserializer::<ValidateBlock>().boxed(),
        pure_stage::register_data_deserializer::<ValidateBlockMsg>().boxed(),
        pure_stage::register_data_deserializer::<SelectChainMsg>().boxed(),
        pure_stage::register_data_deserializer::<AdoptChainMsg>().boxed(),
        pure_stage::register_data_deserializer::<BlockSourceMsg>().boxed(),
        pure_stage::register_data_deserializer::<Tip>().boxed(),
        pure_stage::register_data_deserializer::<amaru_kernel::cardano::network_block::NetworkBlock>().boxed(),
        pure_stage::register_data_deserializer::<Option<(BlockHeader, Option<bool>)>>().boxed(),
        pure_stage::register_data_deserializer::<Option<HeaderHash>>().boxed(),
        pure_stage::register_effect_deserializer::<LoadHeaderEffect>().boxed(),
        pure_stage::register_effect_deserializer::<LoadBlockEffect>().boxed(),
        pure_stage::register_effect_deserializer::<LoadHeaderWithValidityEffect>().boxed(),
        pure_stage::register_effect_deserializer::<LoadFromBestChainEffect>().boxed(),
        pure_stage::register_effect_deserializer::<GetAnchorHashEffect>().boxed(),
        pure_stage::register_effect_deserializer::<ContainsPointEffect>().boxed(),
        pure_stage::register_effect_deserializer::<TipEffect>().boxed(),
        pure_stage::register_effect_deserializer::<RollbackBlockEffect>().boxed(),
        pure_stage::register_effect_deserializer::<ValidateBlockEffect>().boxed(),
        pure_stage::register_effect_deserializer::<RecordMetricsEffect>().boxed(),
    ]
}

pub fn test_prep() -> TestPrep {
    let manager = StageRef::named_for_tests("manager");
    let select_chain = StageRef::named_for_tests("select_chain");
    let block_source = StageRef::named_for_tests("block_source");
    let block_validator = Arc::new(MockBlockValidator::new(Point::Origin));

    let state = ValidateBlock::new(manager.clone(), select_chain.clone(), block_source.clone(), Point::Origin);

    TestPrep {
        state,
        rt: Builder::new_current_thread().build().unwrap(),
        manager,
        select_chain,
        block_source,
        headers: HeaderTree::new(),
        store: Arc::new(InMemConsensusStore::new()),
        block_validator,
    }
}

pub fn setup(prep: &TestPrep, msg: ValidateBlockMsg) -> (SimulationRunning, DeserializerGuards, Logs) {
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
    network.resources().put::<ResourceBlockValidation>(prep.block_validator.clone());
    network.resources().put::<ResourceHasStakePools>(prep.block_validator.clone());

    let vb = network.stage("vb", stage);
    let vb = network.wire_up(vb, prep.state.clone());
    network.preload(&vb, [msg]).unwrap();

    let mut running = network.run();
    running.run_until_blocked_incl_effects(prep.rt.handle());

    (running, guards, logs.logs())
}

pub fn te_validate_block(at_stage: &str, peer: &Peer, point: Point) -> TraceMatch<'static> {
    let ctx = opentelemetry::Context::current();
    TraceEntry::suspend(Effect::external(at_stage, Box::new(ValidateBlockEffect::new(peer, &point, ctx)))).into()
}

pub fn te_record_metrics(at_stage: &str) -> TraceMatch<'_> {
    TraceMatch::Property(
        Box::new(move |e| {
            if let TraceEntry::Suspend(Effect::External { at_stage: at, effect }) = e {
                at.as_str() == at_stage && effect.is::<RecordMetricsEffect>()
            } else {
                false
            }
        }),
        format!("record metrics effect at stage {at_stage}"),
    )
}

pub fn te_ledger_contains(at_stage: &str, point: &Point) -> TraceMatch<'static> {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(ContainsPointEffect::new(point)))).into()
}

pub fn te_ledger_tip(at_stage: &str) -> TraceMatch<'static> {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(TipEffect))).into()
}

pub fn te_get_anchor_hash(at_stage: &str) -> TraceMatch<'static> {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(GetAnchorHashEffect::new()))).into()
}

pub fn te_load_header_with_validity(at_stage: &str, hash: HeaderHash) -> TraceMatch<'static> {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(LoadHeaderWithValidityEffect::new(hash)))).into()
}

pub fn te_load_from_best_chain(at_stage: &str, point: Point) -> TraceMatch<'static> {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(LoadFromBestChainEffect::new(point)))).into()
}

pub fn te_rollback_ledger(at_stage: &str, point: &Point) -> TraceMatch<'static> {
    TraceEntry::suspend(Effect::external(
        at_stage,
        Box::new(RollbackBlockEffect::new(&Peer::new("unknown"), point, opentelemetry::Context::current())),
    ))
    .into()
}

pub fn te_send(from: impl AsRef<str>, to: impl AsRef<str>, msg: impl pure_stage::SendData) -> TraceMatch<'static> {
    TraceEntry::suspend(pure_stage::Effect::send(from, to, Box::new(msg))).into()
}

pub fn te_terminate(at_stage: impl AsRef<str>) -> TraceMatch<'static> {
    TraceEntry::suspend(Effect::Terminate { at_stage: Name::from(at_stage.as_ref()) }).into()
}

pub fn te_terminated(at_stage: impl AsRef<str>, reason: TerminationReason) -> TraceMatch<'static> {
    TraceEntry::Terminated { stage: Name::from(at_stage.as_ref()), reason }.into()
}

pub enum TraceMatch<'a> {
    Literal(TraceEntry),
    Property(Box<dyn Fn(&TraceEntry) -> bool + 'a>, String),
}
impl From<TraceEntry> for TraceMatch<'static> {
    fn from(entry: TraceEntry) -> Self {
        TraceMatch::Literal(entry)
    }
}
impl<'a> PartialEq<TraceEntry> for TraceMatch<'a> {
    fn eq(&self, other: &TraceEntry) -> bool {
        match self {
            TraceMatch::Literal(literal) => literal == other,
            TraceMatch::Property(predicate, _) => predicate(other),
        }
    }
}
impl<'a> PartialEq<TraceMatch<'a>> for TraceEntry {
    fn eq(&self, other: &TraceMatch<'a>) -> bool {
        match other {
            TraceMatch::Literal(literal) => self == literal,
            TraceMatch::Property(predicate, _) => predicate(self),
        }
    }
}
impl<'a> fmt::Debug for TraceMatch<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TraceMatch::Literal(literal) => fmt::Debug::fmt(literal, f),
            TraceMatch::Property(_predicate, description) => f.write_str(description),
        }
    }
}

#[track_caller]
pub fn assert_trace<'a>(running: &SimulationRunning, expected: &[TraceMatch<'a>]) {
    let mut tb = running.trace_buffer().lock();
    for e in tb.iter_entries() {
        if let TraceEntry::InvalidBytes(bytes, value) = e.1 {
            panic!("invalid bytes: {bytes:?}\n\nvalue: {value:?}");
        }
    }
    let trace = tb
        .iter_entries()
        .filter_map(|(_, e)| (!matches!(e, TraceEntry::Resume { .. })).then_some(e))
        .collect::<Vec<_>>();
    tb.clear();
    pretty_assertions::assert_eq!(trace, expected);
}
