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

use amaru_kernel::{BlockHeight, Point};
use amaru_pure_stage::{
    DeserializerGuards, Effect, StageGraph, StageRef,
    simulation::{Run, SimulationBuilder, SimulationRunning},
    trace_buffer::{TraceBuffer, TraceEntry},
};
use tokio::runtime::Runtime;

use super::{BlockSource, BlockSourceMsg, stage};
use crate::stages::{
    peer_selection::PeerSelectionMsg,
    test_utils::{BufferWriter, Logs, install_test_log_capture, start_in_era},
};

pub struct TestPrep {
    pub state: BlockSource,
    pub rt: Runtime,
}

pub fn test_prep(adopted_tip: Point, max_tip_distance: u64) -> TestPrep {
    let invalid_sink = StageRef::named_for_tests("invalid_sink");
    let state = BlockSource::new(adopted_tip, max_tip_distance, invalid_sink);
    TestPrep { state, rt: crate::stages::test_utils::test_runtime() }
}

pub fn register_guards() -> DeserializerGuards {
    vec![
        amaru_pure_stage::register_data_deserializer::<BlockSource>().boxed(),
        amaru_pure_stage::register_data_deserializer::<BlockSourceMsg>().boxed(),
        amaru_pure_stage::register_data_deserializer::<PeerSelectionMsg>().boxed(),
        amaru_pure_stage::register_data_deserializer::<amaru_kernel::Peer>().boxed(),
        amaru_pure_stage::register_data_deserializer::<Point>().boxed(),
        amaru_pure_stage::register_data_deserializer::<amaru_kernel::Point>().boxed(),
        amaru_pure_stage::register_data_deserializer::<BlockHeight>().boxed(),
    ]
}

pub fn setup(prep: &TestPrep, msgs: &[BlockSourceMsg]) -> (SimulationRunning, DeserializerGuards, Logs) {
    let logs = install_test_log_capture(BufferWriter::new());

    let guards = register_guards();

    let mut network = SimulationBuilder::default()
        .with_trace_buffer(TraceBuffer::new_shared(200, 1000000))
        .with_global_epoch_offset(start_in_era().relative_time);
    let bs = network.stage("bs", stage);
    let bs = network.wire_up(bs, prep.state.clone());
    network.preload(&bs, msgs.iter().cloned()).expect("preload");

    let mut running = network.run(prep.rt.handle());
    running.run(Run::skip_and_resolve());

    (running, guards, logs.logs())
}

pub fn te_send(from: impl AsRef<str>, to: impl AsRef<str>, msg: impl amaru_pure_stage::SendData) -> TraceEntry {
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
