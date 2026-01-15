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

use pretty_assertions::Comparison;
use pure_stage::{
    BLACKHOLE_NAME, Effect as Eff, Effects, Instant, Name, OutputEffect, ScheduleId, ScheduleIds,
    SendData, StageGraph, StageRef, StageResponse as Resp, UnknownExternalEffect,
    serde::SendDataValue,
    simulation::SimulationBuilder,
    tokio::TokioBuilder,
    trace_buffer::{TerminationReason, TraceBuffer, TraceEntry as E},
};
use std::sync::Arc;
use std::{collections::BTreeMap, time::Duration};
use tokio::runtime::Runtime;

fn logging() {
    tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()
        .ok();
}

fn group_by_stage(entries: &[E]) -> BTreeMap<&Name, Vec<&E>> {
    let mut map = BTreeMap::<_, Vec<_>>::new();
    for e in entries {
        map.entry(e.at_stage().unwrap_or(&BLACKHOLE_NAME))
            .or_default()
            .push(e);
    }
    map
}

#[track_caller]
#[cfg(test)]
fn assert_equiv(actual1: Vec<E>, expected: &[E]) {
    // permit reorderings between stages and only enforce expected prefix

    let actual = group_by_stage(&actual1);
    let expected = group_by_stage(expected);
    let mut diff = Vec::new();
    for (name, entries) in expected {
        let actual = actual.get(name).map(|e| e.as_slice()).unwrap_or_default();
        if !actual.starts_with(&entries) {
            diff.push((name, Comparison::new(actual, &entries).to_string()));
        }
    }
    if !diff.is_empty() {
        let mut msg = String::new();
        for (name, diff) in diff {
            msg.push_str(&format!("stage {name}:\n{diff}\n"));
        }
        msg.push_str("\n\nactual entries\n\n");
        for entry in actual1.iter() {
            msg.push_str(&format!("{entry:?}\n"));
        }
        panic!("trace entries differ:\n{msg}");
    }
}

#[cfg(test)]
fn run_sim(graph: impl Fn(&mut SimulationBuilder)) -> Vec<E> {
    let rt = Runtime::new().unwrap();
    let trace_buffer = TraceBuffer::new_shared(100, 1_000_000);
    let guard = TraceBuffer::drop_guard(&trace_buffer);

    let mut network = SimulationBuilder::default()
        .with_trace_buffer(trace_buffer.clone())
        .with_epoch_clock();
    graph(&mut network);

    let mut sim = network.run();
    sim.run_until_blocked_incl_effects(rt.handle())
        .assert_terminated("trigger");

    guard.defuse();

    trace_buffer.lock().hydrate_without_timestamps()
}

#[cfg(test)]
fn run_tokio(graph: impl Fn(&mut TokioBuilder)) -> Vec<E> {
    let rt = Runtime::new().unwrap();
    let trace_buffer = TraceBuffer::new_shared(100, 1_000_000);
    let guard = TraceBuffer::drop_guard(&trace_buffer);

    let mut network = TokioBuilder::default()
        .with_trace_buffer(trace_buffer.clone())
        .with_schedule_ids(ScheduleIds::default())
        .with_epoch_clock();
    graph(&mut network);

    let sim = network.run(rt.handle().clone());
    rt.block_on(async move { tokio::time::timeout(Duration::from_secs(3), sim.join()).await })
        .unwrap();

    guard.defuse();

    trace_buffer.lock().hydrate_without_timestamps()
}

fn sr<T>(name: &str) -> StageRef<T> {
    StageRef::named_for_tests(name)
}
fn src<T>(name: &str) -> StageRef<T> {
    StageRef::named_for_tests(name).with_extra_for_tests(Arc::new(()))
}
fn sdv<T: SendData>(value: T) -> Box<dyn SendData> {
    SendDataValue::boxed(&value)
}
fn dur(millis: u64) -> Duration {
    Duration::from_millis(millis)
}
fn b(value: impl SendData) -> Box<dyn SendData> {
    Box::new(value)
}
fn n(name: &str) -> Name {
    Name::from(name)
}

#[test]
fn send() {
    logging();
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
        E::input("stage-1", sdv(3u32)),
        E::resume("stage-1", Resp::Unit),
        E::suspend(Eff::send("stage-1", "trigger-2", false, sdv(6u32))),
        E::state("trigger-2", sdv(())),
        E::input("trigger-2", sdv(6u32)),
        E::resume("trigger-2", Resp::Unit),
        E::suspend(Eff::terminate("trigger-2")),
        E::terminated("trigger-2", TerminationReason::Voluntary),
    ];

    assert_equiv(run_sim(graph), &expected);
    assert_equiv(run_tokio(graph), &expected);
}

#[test]
fn call_then_terminate() {
    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    struct CallMsg(u32, StageRef<u32>);

    logging();
    fn graph(builder: &mut impl StageGraph) {
        let callee = builder.stage("callee", async |_: (), msg: CallMsg, eff| {
            eff.send(&msg.1, msg.0 * 2).await;
        });
        let trigger = builder.stage(
            "trigger",
            async |state: StageRef<CallMsg>, msg: u32, eff| {
                // Call the callee and then terminate after receiving the response
                let response = eff
                    .call(&state, dur(1000), move |cr| CallMsg(msg, cr))
                    .await;
                if let Some(_value) = response {
                    // We received a response, now terminate
                    return eff.terminate().await;
                }
                state
            },
        );
        let callee = builder.wire_up(callee, ());
        let trigger = builder.wire_up(trigger, callee.without_state());
        builder.preload(trigger, [5]).unwrap();
    }
    let expected = [
        E::state("trigger-2", sdv(sr::<CallMsg>("callee-1"))),
        E::input("trigger-2", sdv(5u32)),
        E::resume("trigger-2", Resp::Unit),
        E::suspend(Eff::call(
            "trigger-2",
            "callee-1",
            dur(1000),
            sdv(CallMsg(5u32, src::<u32>("trigger-2"))),
        )),
        E::state("callee-1", sdv(())),
        E::input("callee-1", sdv(CallMsg(5u32, src::<u32>("trigger-2")))),
        E::resume("callee-1", Resp::Unit),
        E::suspend(Eff::send("callee-1", "trigger-2", true, sdv(10u32))),
        E::resume("trigger-2", Resp::CallResponse(sdv(10u32))),
        E::suspend(Eff::terminate("trigger-2")),
    ];

    assert_equiv(run_sim(graph), &expected);
    assert_equiv(run_tokio(graph), &expected);
}

#[test]
#[expect(clippy::wildcard_enum_match_arm)]
fn clock_wait_then_terminate() {
    logging();
    fn graph(builder: &mut impl StageGraph) {
        let trigger = builder.stage("trigger", async |_: (), _msg: u32, eff| {
            eff.clock().await;
            eff.wait(dur(1000)).await;
            eff.terminate().await
        });
        let trigger = builder.wire_up(trigger, ());
        builder.preload(trigger, [1]).unwrap();
    }
    let expected = |start: Instant| {
        [
            E::state("trigger-1", sdv(())),
            E::input("trigger-1", sdv(1u32)),
            E::resume("trigger-1", Resp::Unit),
            E::suspend(Eff::clock("trigger-1")),
            E::resume("trigger-1", Resp::ClockResponse(start)),
            E::suspend(Eff::wait("trigger-1", dur(1000))),
            E::resume("trigger-1", Resp::WaitResponse(start + dur(1000))),
            E::suspend(Eff::terminate("trigger-1")),
        ]
    };

    assert_equiv(run_sim(graph), &expected(*pure_stage::EPOCH));

    let _guard = Instant::with_tolerance_for_test(dur(300));
    let actual = run_tokio(graph);
    let start = actual
        .iter()
        .find_map(|e| match e {
            E::Resume {
                response: Resp::ClockResponse(now),
                ..
            } => Some(*now),
            _ => None,
        })
        .unwrap();
    assert_equiv(actual, &expected(start));
}

#[test]
fn scheduling() {
    logging();
    // without this the always-true comparison of ScheduleIds will fail
    let _guard = pure_stage::register_data_deserializer::<Option<ScheduleId>>();

    fn graph(builder: &mut impl StageGraph) {
        let trigger =
            builder.stage(
                "trigger",
                async |id: Option<ScheduleId>, msg: u32, eff| match msg {
                    0 => {
                        // A large enough delay ensures that those delays won't be effectively passed
                        // when we effectively schedule them with the tokio runtime.
                        // Otherwise they might trigger in the wrong expected order.
                        eff.schedule_after(3, dur(1500)).await;
                        let id = eff.schedule_after(2, dur(1200)).await;
                        eff.schedule_after(1, dur(1100)).await;
                        Some(id)
                    }
                    1 => {
                        eff.cancel_schedule(id.unwrap()).await;
                        None
                    }
                    3 => eff.terminate().await,
                    _ => {
                        panic!("should not be here: {msg}");
                    }
                },
            );
        let trigger = builder.wire_up(trigger, None);
        builder.preload(trigger, [0]).unwrap();
    }
    let schedule_ids = ScheduleIds::default();
    let schedule_id_1 = schedule_ids.next_at(Instant::at_offset(dur(1500)));
    let schedule_id_2 = schedule_ids.next_at(Instant::at_offset(dur(1200)));
    let schedule_id_3 = schedule_ids.next_at(Instant::at_offset(dur(1100)));

    let expected = {
        [
            E::state("trigger-1", b(None::<ScheduleId>)),
            E::input("trigger-1", sdv(0u32)),
            E::resume("trigger-1", Resp::Unit),
            E::suspend(Eff::schedule("trigger-1", sdv(3u32), &schedule_id_1)),
            E::resume("trigger-1", Resp::Unit),
            E::suspend(Eff::schedule("trigger-1", sdv(2u32), &schedule_id_2)),
            E::resume("trigger-1", Resp::Unit),
            E::suspend(Eff::schedule("trigger-1", sdv(1u32), &schedule_id_3)),
            E::resume("trigger-1", Resp::Unit),
            E::state("trigger-1", b(Some(schedule_id_2))),
            E::input("trigger-1", sdv(1u32)),
            E::resume("trigger-1", Resp::Unit),
            E::suspend(Eff::cancel("trigger-1", &schedule_id_2)),
            E::resume("trigger-1", Resp::CancelScheduleResponse(true)),
            E::state("trigger-1", b(None::<ScheduleId>)),
            E::input("trigger-1", sdv(3u32)),
            E::resume("trigger-1", Resp::Unit),
            E::suspend(Eff::terminate("trigger-1")),
            E::terminated("trigger-1", TerminationReason::Voluntary),
        ]
    };

    assert_equiv(run_sim(graph), &expected);

    let _guard = Instant::with_tolerance_for_test(dur(300));
    assert_equiv(run_tokio(graph), &expected);
}

#[test]
fn external_effect() {
    logging();
    fn graph(builder: &mut impl StageGraph) {
        let trigger = builder.stage("trigger", async |_: (), msg: u32, eff| {
            eff.external(OutputEffect::fake(Name::from("output"), msg + 1).0)
                .await;
            eff.terminate().await
        });
        let trigger = builder.wire_up(trigger, ());
        builder.preload(trigger, [1]).unwrap();
    }
    let expected = [
        E::state("trigger-1", sdv(())),
        E::input("trigger-1", sdv(1u32)),
        E::resume("trigger-1", Resp::Unit),
        E::suspend(Eff::external(
            "trigger-1",
            UnknownExternalEffect::boxed(&OutputEffect::fake(Name::from("output"), 2u32).0),
        )),
        E::resume("trigger-1", Resp::ExternalResponse(sdv(()))),
        E::suspend(Eff::terminate("trigger-1")),
    ];
    assert_equiv(run_sim(graph), &expected);
    assert_equiv(run_tokio(graph), &expected);
}

#[test]
fn supervision() {
    logging();
    fn graph(builder: &mut impl StageGraph) {
        async fn child(
            (a, b): (StageRef<u32>, StageRef<u32>),
            msg: u32,
            eff: Effects<u32>,
        ) -> (StageRef<u32>, StageRef<u32>) {
            match msg {
                1 => {
                    // first one stage we supervise
                    let a = eff
                        .stage("a", async |_: (), _: u32, eff| eff.terminate().await)
                        .await;
                    let a = eff.supervise(a, 2);
                    let a = eff.wire_up(a, ()).await;
                    // set the supervision cascade in motion
                    eff.send(&a, 20).await;

                    // second one we don't supervise, whose termination will terminate us
                    let b = eff
                        .stage("b", async |_: (), _: u32, eff| eff.terminate().await)
                        .await;
                    let b = eff.wire_up(b, ()).await;

                    // third which will be aborted by our termination
                    let c = eff
                        .stage("c", async |_: (), _: u32, eff| eff.terminate().await)
                        .await;
                    eff.wire_up(c, ()).await;

                    (a, b)
                }
                2 => {
                    eff.send(&b, 3).await;
                    (a, b)
                }
                _ => {
                    panic!("should not be here: {msg}");
                }
            }
        }

        let trigger = builder.stage("trigger", async |_: (), _msg: u32, eff| {
            let child = eff.stage("child", child).await;
            let child = eff
                .wire_up(child, (StageRef::blackhole(), StageRef::blackhole()))
                .await;
            eff.send(&child, 1).await;
        });
        let trigger = builder.wire_up(trigger, ());
        builder.preload(trigger, [1]).unwrap();
    }
    let expected = [
        E::state("trigger-1", sdv(())),
        E::input("trigger-1", sdv(1u32)),
        E::resume("trigger-1", Resp::Unit),
        E::suspend(Eff::add_stage("trigger-1", "child")),
        E::resume("trigger-1", Resp::AddStageResponse(n("child-2"))),
        E::suspend(Eff::wire_stage(
            "trigger-1",
            "child-2",
            sdv((StageRef::<u32>::blackhole(), StageRef::<u32>::blackhole())),
            None,
        )),
        E::resume("trigger-1", Resp::Unit),
        E::suspend(Eff::send("trigger-1", "child-2", false, sdv(1u32))),
        E::resume("trigger-1", Resp::Unit),
        E::state("trigger-1", sdv(())),
        E::state(
            "child-2",
            sdv((StageRef::<u32>::blackhole(), StageRef::<u32>::blackhole())),
        ),
        E::input("child-2", sdv(1u32)),
        E::resume("child-2", Resp::Unit),
        E::suspend(Eff::add_stage("child-2", "a")),
        E::resume("child-2", Resp::AddStageResponse(n("a-3"))),
        E::suspend(Eff::wire_stage("child-2", "a-3", sdv(()), Some(sdv(2u32)))),
        E::state("a-3", sdv(())),
        E::resume("child-2", Resp::Unit),
        E::suspend(Eff::send("child-2", "a-3", false, sdv(20u32))),
        E::resume("child-2", Resp::Unit),
        E::suspend(Eff::add_stage("child-2", "b")),
        E::resume("child-2", Resp::AddStageResponse(n("b-4"))),
        E::suspend(Eff::wire_stage("child-2", "b-4", sdv(()), None)),
        E::state("b-4", sdv(())),
        E::resume("child-2", Resp::Unit),
        E::suspend(Eff::add_stage("child-2", "c")),
        E::resume("child-2", Resp::AddStageResponse(n("c-5"))),
        E::suspend(Eff::wire_stage("child-2", "c-5", sdv(()), None)),
        E::state("c-5", sdv(())),
        E::resume("child-2", Resp::Unit),
        E::state(
            "child-2",
            sdv((
                StageRef::<u32>::named_for_tests("a-3"),
                StageRef::<u32>::named_for_tests("b-4"),
            )),
        ),
        E::input("a-3", sdv(20u32)),
        E::resume("a-3", Resp::Unit),
        E::suspend(Eff::terminate("a-3")),
        E::terminated("a-3", TerminationReason::Voluntary),
        E::input("child-2", sdv(2u32)),
        E::resume("child-2", Resp::Unit),
        E::suspend(Eff::send("child-2", "b-4", false, sdv(3u32))),
        E::resume("child-2", Resp::Unit),
        E::state(
            "child-2",
            sdv((
                StageRef::<u32>::named_for_tests("a-3"),
                StageRef::<u32>::named_for_tests("b-4"),
            )),
        ),
        E::input("b-4", sdv(3u32)),
        E::resume("b-4", Resp::Unit),
        E::suspend(Eff::terminate("b-4")),
        E::terminated("b-4", TerminationReason::Voluntary),
        E::terminated("child-2", TerminationReason::Supervision(n("b-4"))),
        E::terminated("c-5", TerminationReason::Aborted),
        E::terminated("trigger-1", TerminationReason::Supervision(n("child-2"))),
    ];
    assert_equiv(run_sim(graph), &expected);
    assert_equiv(run_tokio(graph), &expected);
}

#[test]
fn caller_already_terminated() {
    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    enum Msg {
        Start,
        Super,
        Ref(StageRef<u32>),
    }

    logging();
    fn graph(builder: &mut impl StageGraph) {
        let trigger = builder.stage(
            "trigger",
            async |callee_state: StageRef<u32>, msg: Msg, eff| match msg {
                Msg::Start => {
                    let caller = eff
                        .stage(
                            "caller",
                            async |_: (), callee: StageRef<StageRef<u32>>, eff| {
                                eff.call(&callee, dur(1000), move |cr| cr).await;
                                eff.wait(dur(500)).await;
                                eff.terminate().await
                            },
                        )
                        .await;
                    let caller = eff.supervise(caller, Msg::Super);
                    let caller = eff.wire_up(caller, ()).await;

                    let callee = eff
                        .stage(
                            "callee",
                            async |parent: StageRef<StageRef<u32>>, msg: StageRef<u32>, eff| {
                                eff.send(&msg, 5).await;
                                eff.send(&parent, msg).await;
                                parent
                            },
                        )
                        .await;
                    let me = eff.contramap(eff.me_ref(), "parent", Msg::Ref).await;
                    let callee = eff.wire_up(callee, me).await;

                    eff.send(&caller, callee).await;
                    callee_state
                }
                Msg::Super => {
                    eff.send(&callee_state, 6).await;
                    eff.terminate().await
                }
                Msg::Ref(callee) => callee,
            },
        );
        let trigger = builder.wire_up(trigger, StageRef::blackhole());
        builder.preload(trigger, [Msg::Start]).unwrap();
    }

    let expected = [
        E::state("trigger-1", sdv(sr::<u32>(""))),
        E::input("trigger-1", sdv(Msg::Start)),
        E::resume("trigger-1", Resp::Unit),
        E::suspend(Eff::add_stage("trigger-1", "caller")),
        E::resume("trigger-1", Resp::AddStageResponse(n("caller-2"))),
        E::suspend(Eff::wire_stage(
            "trigger-1",
            "caller-2",
            sdv(()),
            Some(sdv(Msg::Super)),
        )),
        E::resume("trigger-1", Resp::Unit),
        E::suspend(Eff::add_stage("trigger-1", "callee")),
        E::resume("trigger-1", Resp::AddStageResponse(n("callee-3"))),
        E::suspend(Eff::contramap("trigger-1", "trigger-1", "parent")),
        E::resume("trigger-1", Resp::ContramapResponse(n("parent-4"))),
        E::suspend(Eff::wire_stage(
            "trigger-1",
            "callee-3",
            sdv(sr::<StageRef<u32>>("parent-4")),
            None,
        )),
        E::resume("trigger-1", Resp::Unit),
        E::suspend(Eff::send(
            "trigger-1",
            "caller-2",
            false,
            sdv(sr::<StageRef<u32>>("callee-3")),
        )),
        E::resume("trigger-1", Resp::Unit),
        E::state("trigger-1", sdv(sr::<u32>(""))),
        E::state("caller-2", sdv(())),
        E::input("caller-2", sdv(sr::<StageRef<u32>>("callee-3"))),
        E::resume("caller-2", Resp::Unit),
        E::suspend(Eff::call(
            "caller-2",
            "callee-3",
            dur(1000),
            sdv(src::<u32>("caller-2")),
        )),
        E::state("callee-3", sdv(sr::<StageRef<u32>>("parent-4"))),
        E::input("callee-3", sdv(src::<u32>("caller-2"))),
        E::resume("callee-3", Resp::Unit),
        E::suspend(Eff::send("callee-3", "caller-2", true, sdv(5u32))),
        E::resume("callee-3", Resp::Unit),
        E::suspend(Eff::send(
            "callee-3",
            "parent-4",
            false,
            sdv(src::<u32>("caller-2")),
        )),
        E::input("trigger-1", sdv(Msg::Ref(src::<u32>("caller-2")))),
        E::resume("trigger-1", Resp::Unit),
        E::state("trigger-1", sdv(src::<u32>("caller-2"))),
        E::input("trigger-1", sdv(Msg::Super)),
        E::resume("trigger-1", Resp::Unit),
        E::suspend(Eff::send("trigger-1", "caller-2", true, sdv(6u32))),
        E::resume("trigger-1", Resp::Unit),
        E::suspend(Eff::terminate("trigger-1")),
        E::terminated("trigger-1", TerminationReason::Voluntary),
    ];
    assert_equiv(run_sim(graph), &expected);
    assert_equiv(run_tokio(graph), &expected);
}
