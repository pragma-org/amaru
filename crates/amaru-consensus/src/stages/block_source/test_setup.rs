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

use amaru_kernel::{BlockHeight, Tip};
use pure_stage::{
    DeserializerGuards, Effect, StageGraph, StageRef,
    simulation::{SimulationBuilder, SimulationRunning},
    trace_buffer::{TraceBuffer, TraceEntry},
};
use tokio::runtime::{Builder, Runtime};
use tracing::Level;
use tracing_subscriber::util::SubscriberInitExt;

use super::{BlockSource, BlockSourceMsg, stage};
use crate::stages::{
    peer_selection::PeerSelectionMsg,
    test_utils::{BufferWriter, Logs},
};

pub struct TestPrep {
    pub state: BlockSource,
    pub rt: Runtime,
}

pub fn test_prep(adopted_tip: Tip, max_tip_distance: u64) -> TestPrep {
    let invalid_sink = StageRef::named_for_tests("invalid_sink");
    let state = BlockSource::new(adopted_tip, max_tip_distance, invalid_sink);
    TestPrep { state, rt: Builder::new_current_thread().build().unwrap() }
}

pub fn register_guards() -> DeserializerGuards {
    vec![
        pure_stage::register_data_deserializer::<BlockSource>().boxed(),
        pure_stage::register_data_deserializer::<BlockSourceMsg>().boxed(),
        pure_stage::register_data_deserializer::<PeerSelectionMsg>().boxed(),
        pure_stage::register_data_deserializer::<amaru_kernel::Peer>().boxed(),
        pure_stage::register_data_deserializer::<Tip>().boxed(),
        pure_stage::register_data_deserializer::<amaru_kernel::Point>().boxed(),
        pure_stage::register_data_deserializer::<BlockHeight>().boxed(),
    ]
}

pub fn setup(prep: &TestPrep, msgs: &[BlockSourceMsg]) -> (SimulationRunning, DeserializerGuards, Logs) {
    let writer = BufferWriter::new();
    let mut logs = writer.clone();

    let sub = tracing_subscriber::fmt()
        .with_max_level(Level::DEBUG)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .set_default();
    logs.set_guard(sub);

    let guards = register_guards();

    let mut network = SimulationBuilder::default().with_trace_buffer(TraceBuffer::new_shared(200, 1000000));
    let bs = network.stage("bs", stage);
    let bs = network.wire_up(bs, prep.state.clone());
    network.preload(&bs, msgs.iter().cloned()).expect("preload");

    let mut running = network.run();
    running.run_until_blocked_incl_effects(prep.rt.handle());

    (running, guards, logs.logs())
}

pub fn te_send(from: impl AsRef<str>, to: impl AsRef<str>, msg: impl pure_stage::SendData) -> TraceEntry {
    TraceEntry::suspend(Effect::send(from, to, Box::new(msg)))
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
