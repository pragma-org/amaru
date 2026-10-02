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

#![expect(clippy::unwrap_used, clippy::expect_used)]

//! The same stage graph, driven by the simulation runtime and by the Tokio runtime.
//!
//! Simulation time is virtual. Tokio assertions use wall-clock bounds around the same
//! deadlines so a runtime that waits forever, or that returns before the deadline, fails.

use std::{pin::pin, time::Duration};

use amaru_pure_stage::{
    Receiver, Sender, StageGraph, StageRef, assert_trace_contains,
    simulation::{Run, SimulationBuilder},
    tm_call,
    tokio::TokioBuilder,
    trace_buffer::TraceBuffer,
};
use futures_util::StreamExt;
use tokio::time::timeout;

const CALL_TIMEOUT: Duration = Duration::from_millis(100);
const HOLD: Duration = Duration::from_secs(1);
const REPLY_TIMEOUT: Duration = Duration::from_millis(200);

#[derive(Clone, Copy)]
enum Runtime {
    Simulation,
    Tokio,
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
enum Mail {
    Occupy,
    Filler(u8),
    Ping(StageRef<u32>),
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
enum Out {
    Holding,
    Timeout,
    Responded(u32),
    Saw(u8),
    Dropped,
    Elapsed { ms: u64, timed_out: bool },
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct Caller {
    callee: StageRef<Mail>,
    out: StageRef<Out>,
}

struct Ends {
    callee: StageRef<Mail>,
    caller: StageRef<u8>,
    to_callee: Sender<Mail>,
    to_caller: Sender<u8>,
    out: Receiver<Out>,
}

fn test_runtime() -> &'static tokio::runtime::Runtime {
    static RT: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RT.get_or_init(|| tokio::runtime::Runtime::new().expect("tokio runtime"))
}

fn register() -> amaru_pure_stage::DeserializerGuards {
    vec![
        Box::new(amaru_pure_stage::register_data_deserializer::<Mail>()),
        Box::new(amaru_pure_stage::register_data_deserializer::<Out>()),
        Box::new(amaru_pure_stage::register_data_deserializer::<Caller>()),
        Box::new(amaru_pure_stage::register_data_deserializer::<u8>()),
        Box::new(amaru_pure_stage::register_data_deserializer::<u32>()),
    ]
}

fn install_blocked_call(graph: &mut impl StageGraph) -> Ends {
    let callee = graph.stage("callee", async |out: StageRef<Out>, msg: Mail, eff| {
        match msg {
            Mail::Occupy => {
                eff.send(&out, Out::Holding).await;
                eff.wait(HOLD).await;
            }
            Mail::Filler(n) => eff.send(&out, Out::Saw(n)).await,
            Mail::Ping(reply) => {
                eff.send(&out, Out::Saw(9)).await;
                eff.send(&reply, 1u32).await;
            }
        }
        out
    });
    let caller = graph.stage("caller", async |st: Caller, _msg: u8, eff| {
        match eff.call(&st.callee, CALL_TIMEOUT, Mail::Ping).await {
            Some(value) => eff.send(&st.out, Out::Responded(value)).await,
            None => eff.send(&st.out, Out::Timeout).await,
        }
        st
    });
    let (out, rx) = graph.output("out", 16);
    let callee_ref = callee.sender();
    let caller_ref = caller.sender();
    graph.wire_up(callee, out.clone());
    graph.wire_up(caller, Caller { callee: callee_ref.clone(), out });
    let to_callee = graph.input(&callee_ref);
    let to_caller = graph.input(&caller_ref);
    Ends { callee: callee_ref, caller: caller_ref, to_callee, to_caller, out: rx }
}

fn install_dropped_reply(graph: &mut impl StageGraph) -> Ends {
    let callee = graph.stage("callee", async |out: StageRef<Out>, msg: Mail, eff| {
        if let Mail::Ping(_reply) = msg {
            eff.send(&out, Out::Dropped).await;
        }
        out
    });
    let caller = graph.stage("caller", async |st: Caller, _msg: u8, eff| {
        let started = eff.clock().await;
        let response = eff.call(&st.callee, REPLY_TIMEOUT, Mail::Ping).await;
        let elapsed = eff.clock().await.checked_since(started).unwrap_or(Duration::ZERO);
        eff.send(
            &st.out,
            Out::Elapsed { ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX), timed_out: response.is_none() },
        )
        .await;
        st
    });
    let (out, rx) = graph.output("out", 8);
    let callee_ref = callee.sender();
    let caller_ref = caller.sender();
    graph.wire_up(callee, out.clone());
    graph.wire_up(caller, Caller { callee: callee_ref.clone(), out });
    let to_callee = graph.input(&callee_ref);
    let to_caller = graph.input(&caller_ref);
    Ends { callee: callee_ref, caller: caller_ref, to_callee, to_caller, out: rx }
}

fn install_late_reply(graph: &mut impl StageGraph) -> Ends {
    let callee = graph.stage("callee", async |out: StageRef<Out>, msg: Mail, eff| {
        if let Mail::Ping(reply) = msg {
            eff.wait(HOLD).await;
            eff.send(&reply, 1u32).await;
            eff.send(&out, Out::Saw(1)).await;
        }
        out
    });
    let caller = graph.stage("caller", async |st: Caller, _msg: u8, eff| {
        match eff.call(&st.callee, CALL_TIMEOUT, Mail::Ping).await {
            Some(value) => eff.send(&st.out, Out::Responded(value)).await,
            None => eff.send(&st.out, Out::Timeout).await,
        }
        st
    });
    let (out, rx) = graph.output("out", 8);
    let callee_ref = callee.sender();
    let caller_ref = caller.sender();
    graph.wire_up(callee, out.clone());
    graph.wire_up(caller, Caller { callee: callee_ref.clone(), out });
    let to_callee = graph.input(&callee_ref);
    let to_caller = graph.input(&caller_ref);
    Ends { callee: callee_ref, caller: caller_ref, to_callee, to_caller, out: rx }
}

fn install_cancel(graph: &mut impl StageGraph) -> (StageRef<u8>, Sender<u8>, Receiver<u8>) {
    let stage = graph.stage("sched", async |out: StageRef<u8>, msg: u8, eff| {
        if msg == 0 {
            let id = eff.schedule_after(1u8, Duration::from_secs(30)).await;
            if eff.cancel_schedule(id).await {
                eff.schedule_after(2u8, Duration::from_secs(30)).await;
                eff.send(&out, 1u8).await;
            } else {
                eff.send(&out, 0u8).await;
            }
        }
        out
    });
    let (out, rx) = graph.output("out", 4);
    let stage_ref = stage.sender();
    graph.wire_up(stage, out);
    let tx = graph.input(&stage_ref);
    (stage_ref, tx, rx)
}

fn call_enqueue_timeout(runtime: Runtime) {
    let _guards = register();
    match runtime {
        Runtime::Simulation => {
            let trace_buffer = TraceBuffer::new_shared(100, 1_000_000);
            let mut network = SimulationBuilder::default().with_mailbox_size(1).with_trace_buffer(trace_buffer);
            let mut ends = install_blocked_call(&mut network);
            let mut sim = network.run(test_runtime().handle());

            sim.enqueue_msg(&ends.callee, [Mail::Occupy]);
            sim.run(Run::default()).assert_sleeping();
            assert_eq!(ends.out.try_next(), Some(Out::Holding));

            sim.enqueue_msg(&ends.callee, [Mail::Filler(1)]);
            assert_eq!(sim.mailbox_len(&ends.callee), 1);

            let started = sim.now();
            sim.enqueue_msg(&ends.caller, [0]);
            sim.run(Run::until(started + CALL_TIMEOUT));
            assert_eq!(ends.out.try_next(), Some(Out::Timeout));
            assert_eq!(sim.mailbox_len(&ends.callee), 1, "the timed-out call must not occupy the mailbox");

            sim.run(Run::skip_wakeups()).assert_idle();
            assert_eq!(ends.out.drain().collect::<Vec<_>>(), vec![Out::Saw(1)]);
            assert_trace_contains(
                &sim,
                &[tm_call(ends.caller.name().as_str(), ends.callee.name().as_str(), CALL_TIMEOUT)],
            );
        }
        Runtime::Tokio => {
            let rt = test_runtime();
            let mut network = TokioBuilder::default().with_mailbox_size(1);
            let mut ends = install_blocked_call(&mut network);
            let running = network.run(rt.handle().clone());
            rt.block_on(async move {
                ends.to_callee.send(Mail::Occupy).await.unwrap();
                assert_eq!(timeout(Duration::from_secs(1), ends.out.next()).await.unwrap(), Some(Out::Holding));
                ends.to_callee.send(Mail::Filler(1)).await.unwrap();
                ends.to_caller.send(0).await.unwrap();
                let call = timeout(Duration::from_millis(400), ends.out.next()).await;
                assert_eq!(call.expect("call must time out while enqueue is blocked"), Some(Out::Timeout));
                assert_eq!(timeout(Duration::from_secs(2), ends.out.next()).await.unwrap(), Some(Out::Saw(1)));
                let late = timeout(Duration::from_millis(50), ends.out.next()).await;
                assert!(late.is_err(), "abandoned call must not be delivered, got {late:?}");
            });
            running.abort();
        }
    }
}

fn elapsed_report(msgs: &[Out]) -> (u64, bool) {
    msgs.iter()
        .find_map(|msg| match msg {
            Out::Elapsed { ms, timed_out } => Some((*ms, *timed_out)),
            Out::Holding | Out::Timeout | Out::Responded(_) | Out::Saw(_) | Out::Dropped => None,
        })
        .expect("elapsed report")
}

fn call_dropped_reply_waits_for_deadline(runtime: Runtime) {
    let _guards = register();
    match runtime {
        Runtime::Simulation => {
            let mut network = SimulationBuilder::default();
            let mut ends = install_dropped_reply(&mut network);
            let mut sim = network.run(test_runtime().handle());
            let started = sim.now();
            sim.enqueue_msg(&ends.caller, [0]);
            sim.run(Run::skip_wakeups()).assert_idle();
            let msgs = ends.out.drain().collect::<Vec<_>>();
            assert!(msgs.contains(&Out::Dropped));
            assert_eq!(elapsed_report(&msgs), (u64::try_from(REPLY_TIMEOUT.as_millis()).unwrap(), true));
            assert_eq!(sim.now().checked_since(started), Some(REPLY_TIMEOUT));
        }
        Runtime::Tokio => {
            let rt = test_runtime();
            let mut network = TokioBuilder::default();
            let mut ends = install_dropped_reply(&mut network);
            let running = network.run(rt.handle().clone());
            rt.block_on(async move {
                ends.to_caller.send(0).await.unwrap();
                let mut msgs = Vec::new();
                let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
                while msgs.iter().all(|msg| !matches!(msg, Out::Elapsed { .. })) {
                    let msg = tokio::time::timeout_at(deadline, ends.out.next())
                        .await
                        .expect("call must finish")
                        .expect("output closed");
                    msgs.push(msg);
                }
                assert!(msgs.contains(&Out::Dropped));
                let (ms, timed_out) = elapsed_report(&msgs);
                assert!(timed_out);
                assert!(ms >= 100, "dropped reply must not complete the call early, elapsed {ms}ms");
                assert!(ms < 2_000, "elapsed {ms}ms");
            });
            running.abort();
        }
    }
}

fn call_late_reply_is_ignored(runtime: Runtime) {
    let _guards = register();
    match runtime {
        Runtime::Simulation => {
            let mut network = SimulationBuilder::default();
            let mut ends = install_late_reply(&mut network);
            let mut sim = network.run(test_runtime().handle());
            sim.enqueue_msg(&ends.caller, [0]);
            let started = sim.now();
            sim.run(Run::until(started + CALL_TIMEOUT));
            assert_eq!(ends.out.try_next(), Some(Out::Timeout));
            sim.run(Run::skip_wakeups()).assert_idle();
            assert_eq!(ends.out.drain().collect::<Vec<_>>(), vec![Out::Saw(1)]);
        }
        Runtime::Tokio => {
            let rt = test_runtime();
            let mut network = TokioBuilder::default();
            let mut ends = install_late_reply(&mut network);
            let running = network.run(rt.handle().clone());
            rt.block_on(async move {
                ends.to_caller.send(0).await.unwrap();
                assert_eq!(
                    timeout(Duration::from_millis(400), ends.out.next()).await.expect("response timeout"),
                    Some(Out::Timeout)
                );
                assert_eq!(timeout(Duration::from_secs(2), ends.out.next()).await.unwrap(), Some(Out::Saw(1)));
            });
            running.abort();
        }
    }
}

fn cancel_frees_priority_slot(runtime: Runtime) {
    let _guards = register();
    match runtime {
        Runtime::Simulation => {
            let mut network = SimulationBuilder::default().with_priority_mailbox_size(1);
            let (stage, _tx, mut out) = install_cancel(&mut network);
            let mut sim = network.run(test_runtime().handle());
            sim.enqueue_msg(&stage, [0]);
            sim.run(Run::default()).assert_sleeping();
            assert_eq!(out.try_next(), Some(1));
        }
        Runtime::Tokio => {
            let rt = test_runtime();
            let mut network = TokioBuilder::default().with_priority_mailbox_size(1);
            let (_stage, tx, mut out) = install_cancel(&mut network);
            let running = network.run(rt.handle().clone());
            let join_on = running.clone();
            rt.block_on(async move {
                tx.send(0).await.unwrap();
                let mut join = pin!(join_on.join());
                tokio::select! {
                    msg = out.next() => assert_eq!(msg, Some(1)),
                    result = &mut join => panic!("graph stopped before the reschedule completed: {result:?}"),
                }
            });
            running.abort();
        }
    }
}

#[test]
fn call_enqueue_timeout_simulation() {
    call_enqueue_timeout(Runtime::Simulation);
}

#[test]
fn call_enqueue_timeout_tokio() {
    call_enqueue_timeout(Runtime::Tokio);
}

#[test]
fn call_dropped_reply_waits_for_deadline_simulation() {
    call_dropped_reply_waits_for_deadline(Runtime::Simulation);
}

#[test]
fn call_dropped_reply_waits_for_deadline_tokio() {
    call_dropped_reply_waits_for_deadline(Runtime::Tokio);
}

#[test]
fn call_late_reply_is_ignored_simulation() {
    call_late_reply_is_ignored(Runtime::Simulation);
}

#[test]
fn call_late_reply_is_ignored_tokio() {
    call_late_reply_is_ignored(Runtime::Tokio);
}

#[test]
fn cancel_frees_priority_slot_simulation() {
    cancel_frees_priority_slot(Runtime::Simulation);
}

#[test]
fn cancel_frees_priority_slot_tokio() {
    cancel_frees_priority_slot(Runtime::Tokio);
}
