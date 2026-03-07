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

use amaru_kernel::{BlockHeader, HeaderHash, TESTNET_ERA_HISTORY, Tip, make_header};
use amaru_ouroboros::ConnectionId;
use amaru_ouroboros_traits::{
    ChainStore,
    can_validate_blocks::{CanValidateHeaders, HeaderValidationError, mock::MockCanValidateHeaders},
    in_memory_consensus_store::InMemConsensusStore,
};
use amaru_protocols::{
    chainsync::InitiatorMessage,
    store_effects::{HasHeaderEffect, LoadHeaderEffect, ResourceHeaderStore, StoreHeaderEffect},
};
use anyhow::anyhow;
use opentelemetry::Context;
use pure_stage::{
    DeserializerGuards, Effect, SendData, StageGraph, StageRef,
    simulation::{SimulationBuilder, SimulationRunning},
    trace_buffer::{TraceBuffer, TraceEntry},
};
use tokio::runtime::{Builder, Handle, Runtime};
use tracing::Level;
use tracing_subscriber::util::SubscriberInitExt;

use super::*;
use crate::{
    effects::{ResourceHeaderValidation, ValidateHeaderEffect},
    stages::test_utils::{BufferWriter, Logs},
};

pub fn build_store(headers: &[BlockHeader]) -> Arc<InMemConsensusStore<BlockHeader>> {
    let store = Arc::new(InMemConsensusStore::new());
    for header in headers {
        store.store_header(header).unwrap();
    }
    store
}

/// Bundles state, runtime, handler, conn_id, and three linked headers for tests.
pub struct TestPrep {
    pub state: TrackPeers,
    pub rt: Runtime,
    pub handler: StageRef<InitiatorMessage>,
    pub conn_id: ConnectionId,
    /// Three linked headers: [h1, h2, h3] with h1 parent None, h2 parent h1, h3 parent h2.
    pub headers: [BlockHeader; 3],
}

impl TestPrep {
    pub fn rt_handle(&self) -> Handle {
        self.rt.handle().clone()
    }
}

/// Creates basic state, runtime, handler, conn_id, and three properly linked headers for tests.
pub fn test_prep() -> TestPrep {
    let state = TrackPeers::new(
        TESTNET_ERA_HISTORY.clone(),
        StageRef::named_for_tests("manager"),
        StageRef::named_for_tests("downstream"),
    );
    let rt = Builder::new_current_thread().build().unwrap();
    let handler = StageRef::<InitiatorMessage>::named_for_tests("handler");
    let conn_id = ConnectionId::initial();
    let h1 = make_block_header(1, 1, None);
    let h2 = make_block_header(2, 2, Some(h1.hash()));
    let h3 = make_block_header(3, 3, Some(h2.hash()));
    TestPrep { state, rt, handler, conn_id, headers: [h1, h2, h3] }
}

pub fn make_block_header(block_number: u64, slot: u64, parent: Option<HeaderHash>) -> BlockHeader {
    BlockHeader::from(make_header(block_number, slot, parent))
}

pub fn te_validate_header(at_stage: &str, header: BlockHeader) -> TraceEntry {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(ValidateHeaderEffect::new(&header, Context::new()))))
}

pub fn te_load_header(at_stage: &str, hash: HeaderHash) -> TraceEntry {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(LoadHeaderEffect::new(hash))))
}

pub fn te_has_header(at_stage: &str, hash: HeaderHash) -> TraceEntry {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(HasHeaderEffect::new(hash))))
}

pub fn te_store_header(at_stage: &str, header: BlockHeader) -> TraceEntry {
    TraceEntry::suspend(Effect::external(at_stage, Box::new(StoreHeaderEffect::new(header))))
}

pub fn te_send(from: impl AsRef<str>, to: impl AsRef<str>, msg: impl SendData) -> TraceEntry {
    TraceEntry::suspend(pure_stage::Effect::send(from, to, Box::new(msg)))
}

fn register_guards() -> DeserializerGuards {
    vec![
        pure_stage::register_data_deserializer::<TrackPeers>().boxed(),
        pure_stage::register_data_deserializer::<TrackPeersMsg>().boxed(),
        pure_stage::register_data_deserializer::<chainsync::InitiatorMessage>().boxed(),
        pure_stage::register_data_deserializer::<ManagerMessage>().boxed(),
        pure_stage::register_data_deserializer::<Tip>().boxed(),
        pure_stage::register_data_deserializer::<(Tip, Point)>().boxed(),
        pure_stage::register_effect_deserializer::<LoadHeaderEffect>().boxed(),
        pure_stage::register_effect_deserializer::<HasHeaderEffect>().boxed(),
        pure_stage::register_effect_deserializer::<StoreHeaderEffect>().boxed(),
        pure_stage::register_effect_deserializer::<ValidateHeaderEffect>().boxed(),
    ]
}

pub fn setup(
    rt: &Handle,
    state: TrackPeers,
    msg: TrackPeersMsg,
    store: Arc<InMemConsensusStore<BlockHeader>>,
) -> (SimulationRunning, DeserializerGuards, Logs) {
    setup_with_validation(rt, state, msg, store, Arc::new(MockCanValidateHeaders))
}

pub fn setup_with_validation(
    rt: &Handle,
    state: TrackPeers,
    msg: TrackPeersMsg,
    store: Arc<InMemConsensusStore<BlockHeader>>,
    validation: Arc<dyn CanValidateHeaders + Send + Sync>,
) -> (SimulationRunning, DeserializerGuards, Logs) {
    let writer = BufferWriter::new();
    let mut logs = writer.clone();

    // set the tracing subscriber for the current thread
    let sub = tracing_subscriber::fmt()
        .with_max_level(Level::DEBUG)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .set_default();
    logs.set_guard(sub);

    let guards = register_guards();

    let mut network = SimulationBuilder::default().with_trace_buffer(TraceBuffer::new_shared(100, 1000000));
    network.resources().put::<ResourceHeaderStore>(store.clone());
    network.resources().put::<ResourceHeaderValidation>(validation);

    let tp = network.stage("tp", stage);
    let tp = network.wire_up(tp, state);
    network.preload(&tp, [msg]).unwrap();

    let mut running = network.run();
    running.run_until_blocked_incl_effects(rt);

    (running, guards, logs.logs())
}

pub struct FailingHeaderValidation;

impl CanValidateHeaders for FailingHeaderValidation {
    fn validate_header(&self, _header: &BlockHeader) -> Result<(), HeaderValidationError> {
        Err(HeaderValidationError::new(anyhow!("header validation failed: booyah!")))
    }
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
