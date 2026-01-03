// Copyright 2025 PRAGMA
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

use pure_stage::{
    Effect as Eff, SendData, StageGraph, StageRef, StageResponse as Resp,
    serde::SendDataValue,
    simulation::SimulationBuilder,
    tokio::TokioBuilder,
    trace_buffer::{TraceBuffer, TraceEntry as E},
};
use std::time::Duration;
use tokio::runtime::Runtime;

fn run_sim(graph: impl Fn(&mut SimulationBuilder)) -> Vec<E> {
    let rt = Runtime::new().unwrap();
    let trace_buffer = TraceBuffer::new_shared(100, 1_000_000);
    let guard = TraceBuffer::drop_guard(&trace_buffer);

    let mut network = SimulationBuilder::default().with_trace_buffer(trace_buffer.clone());
    graph(&mut network);

    let mut sim = network.run();
    sim.run_until_blocked_incl_effects(rt.handle())
        .assert_terminated("trigger");

    guard.defuse();

    trace_buffer.lock().hydrate_without_timestamps()
}

fn run_tokio(graph: impl Fn(&mut TokioBuilder)) -> Vec<E> {
    let rt = Runtime::new().unwrap();
    let trace_buffer = TraceBuffer::new_shared(100, 1_000_000);
    let guard = TraceBuffer::drop_guard(&trace_buffer);

    let mut network = TokioBuilder::default().with_trace_buffer(trace_buffer.clone());
    graph(&mut network);

    let sim = network.run(rt.handle().clone());
    rt.block_on(async move { tokio::time::timeout(Duration::from_secs(10), sim.join()).await })
        .unwrap();

    guard.defuse();

    trace_buffer.lock().hydrate_without_timestamps()
}

fn sr<T>(name: &str) -> StageRef<T> {
    StageRef::named_for_tests(name)
}
fn sdv<T: SendData>(value: T) -> Box<dyn SendData> {
    SendDataValue::boxed(&value)
}

#[test]
fn basic() {
    fn graph(builder: &mut impl StageGraph) {
        let stage = builder.stage("stage", async |state: StageRef<u32>, msg: u32, eff| {
            eff.send(&state, msg * 2).await;
            state
        });
        let trigger = builder.stage("trigger", async |_: (), _msg: u32, eff| {
            eff.terminate().await
        });
        let trigger = builder.wire_up(trigger, ());
        let stage = builder.wire_up(stage, trigger.without_state());
        builder.preload(stage, [3]).unwrap();
    }
    let expected = [
        E::state("stage-1", sdv(sr::<u32>("trigger-2"))),
        E::state("trigger-2", sdv(())),
        E::input("stage-1", sdv(3u32)),
        E::resume("stage-1", Resp::Unit),
        E::suspend(Eff::send("stage-1", "trigger-2", false, sdv(6u32))),
        E::input("trigger-2", sdv(6u32)),
        E::resume("trigger-2", Resp::Unit),
        E::suspend(Eff::terminate("trigger-2")),
    ];

    let trace = run_sim(graph);
    pretty_assertions::assert_eq!(trace, expected);

    let trace = run_tokio(graph);
    pretty_assertions::assert_eq!(trace, expected);
}
