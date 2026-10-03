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
//! Simulation time is virtual. Tokio arms run on a paused clock and advance it with
//! `tokio::time::sleep`, so a runtime that waits forever, or that returns before the
//! deadline, fails without depending on wall-clock timing.

use std::{panic::AssertUnwindSafe, time::Duration};

use amaru_pure_stage::{
    Receiver, Sender, StageGraph, StageRef, assert_trace_contains,
    simulation::{Run, SimulationBuilder},
    tm_call,
    tokio::TokioBuilder,
    trace_buffer::TraceBuffer,
};

const CALL_TIMEOUT: Duration = Duration::from_millis(100);
const HOLD: Duration = Duration::from_secs(1);
const REPLY_TIMEOUT: Duration = Duration::from_millis(200);
const SLOT_WAIT: Duration = Duration::from_millis(40);
const LONG_CALL: Duration = Duration::from_secs(5);

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

fn paused_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().start_paused(true).build().expect("paused runtime")
}

async fn settle() {
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
}

fn register() -> amaru_pure_stage::DeserializerGuards {
    vec![
        Box::new(amaru_pure_stage::register_data_deserializer::<Mail>()),
        Box::new(amaru_pure_stage::register_data_deserializer::<Out>()),
        Box::new(amaru_pure_stage::register_data_deserializer::<Caller>()),
        Box::new(amaru_pure_stage::register_data_deserializer::<ParentMsg>()),
        Box::new(amaru_pure_stage::register_data_deserializer::<Kick>()),
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
            let rt = paused_runtime();
            let mut network = TokioBuilder::default().with_mailbox_size(1);
            let mut ends = install_blocked_call(&mut network);
            let running = network.run(rt.handle().clone());
            rt.block_on(async move {
                ends.to_callee.send(Mail::Occupy).await.unwrap();
                settle().await;
                assert_eq!(ends.out.try_next(), Some(Out::Holding));
                ends.to_callee.send(Mail::Filler(1)).await.unwrap();
                ends.to_caller.send(0).await.unwrap();
                settle().await;
                tokio::time::sleep(CALL_TIMEOUT).await;
                settle().await;
                assert_eq!(ends.out.try_next(), Some(Out::Timeout), "call must time out while enqueue is blocked");
                tokio::time::sleep(HOLD).await;
                settle().await;
                assert_eq!(ends.out.try_next(), Some(Out::Saw(1)));
                assert_eq!(ends.out.try_next(), None, "abandoned call must not be delivered");
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
            let rt = paused_runtime();
            let mut network = TokioBuilder::default();
            let mut ends = install_dropped_reply(&mut network);
            let running = network.run(rt.handle().clone());
            rt.block_on(async move {
                let started = tokio::time::Instant::now();
                ends.to_caller.send(0).await.unwrap();
                settle().await;
                assert_eq!(ends.out.try_next(), Some(Out::Dropped));
                tokio::time::sleep(REPLY_TIMEOUT - Duration::from_millis(1)).await;
                assert_eq!(ends.out.try_next(), None, "dropped reply must not complete the call early");
                tokio::time::sleep(Duration::from_millis(1)).await;
                settle().await;
                let waited = tokio::time::Instant::now().saturating_duration_since(started);
                let report = ends.out.try_next().expect("elapsed report");
                let (_stage_ms, timed_out) = elapsed_report(&[report]);
                assert!(timed_out);
                assert_eq!(waited, REPLY_TIMEOUT, "dropped reply must wait out the deadline");
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
            let rt = paused_runtime();
            let mut network = TokioBuilder::default();
            let mut ends = install_late_reply(&mut network);
            let running = network.run(rt.handle().clone());
            rt.block_on(async move {
                ends.to_caller.send(0).await.unwrap();
                settle().await;
                tokio::time::sleep(CALL_TIMEOUT).await;
                settle().await;
                assert_eq!(ends.out.try_next(), Some(Out::Timeout));
                tokio::time::sleep(HOLD).await;
                settle().await;
                assert_eq!(ends.out.try_next(), Some(Out::Saw(1)));
                assert_eq!(ends.out.try_next(), None);
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
            let rt = paused_runtime();
            let mut network = TokioBuilder::default().with_priority_mailbox_size(1);
            let (_stage, tx, mut out) = install_cancel(&mut network);
            let running = network.run(rt.handle().clone());
            rt.block_on(async move {
                tx.send(0).await.unwrap();
                settle().await;
                assert_eq!(out.try_next(), Some(1));
            });
            running.abort();
        }
    }
}

fn install_delivered_call(graph: &mut impl StageGraph) -> Ends {
    let callee = graph.stage("callee", async |out: StageRef<Out>, msg: Mail, eff| {
        match msg {
            Mail::Occupy => {
                eff.send(&out, Out::Holding).await;
                eff.wait(SLOT_WAIT).await;
            }
            Mail::Filler(n) => eff.send(&out, Out::Saw(n)).await,
            Mail::Ping(reply) => {
                eff.send(&out, Out::Saw(9)).await;
                eff.send(&reply, 7u32).await;
            }
        }
        out
    });
    let caller = graph.stage("caller", async |st: Caller, _msg: u8, eff| {
        match eff.call(&st.callee, LONG_CALL, Mail::Ping).await {
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

fn call_delivers_when_slot_frees(runtime: Runtime) {
    let _guards = register();
    match runtime {
        Runtime::Simulation => {
            let mut network = SimulationBuilder::default().with_mailbox_size(1);
            let mut ends = install_delivered_call(&mut network);
            let mut sim = network.run(test_runtime().handle());
            sim.enqueue_msg(&ends.callee, [Mail::Occupy]);
            sim.run(Run::default()).assert_sleeping();
            assert_eq!(ends.out.try_next(), Some(Out::Holding));
            sim.enqueue_msg(&ends.callee, [Mail::Filler(1)]);
            sim.enqueue_msg(&ends.caller, [0]);
            sim.run(Run::skip_wakeups()).assert_idle();
            assert_eq!(ends.out.drain().collect::<Vec<_>>(), vec![Out::Saw(1), Out::Saw(9), Out::Responded(7)]);
        }
        Runtime::Tokio => {
            let rt = paused_runtime();
            let mut network = TokioBuilder::default().with_mailbox_size(1);
            let mut ends = install_delivered_call(&mut network);
            let running = network.run(rt.handle().clone());
            rt.block_on(async move {
                ends.to_callee.send(Mail::Occupy).await.unwrap();
                settle().await;
                assert_eq!(ends.out.try_next(), Some(Out::Holding));
                ends.to_callee.send(Mail::Filler(1)).await.unwrap();
                ends.to_caller.send(0).await.unwrap();
                tokio::time::sleep(SLOT_WAIT).await;
                settle().await;
                assert_eq!(ends.out.drain().collect::<Vec<_>>(), vec![Out::Saw(1), Out::Saw(9), Out::Responded(7)]);
            });
            running.abort();
        }
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
enum Note {
    Saw(u8),
    Sent(u8),
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct SenderSt {
    dest: StageRef<u8>,
    out: StageRef<Note>,
}

fn register_notes() -> amaru_pure_stage::DeserializerGuards {
    vec![
        Box::new(amaru_pure_stage::register_data_deserializer::<Note>()),
        Box::new(amaru_pure_stage::register_data_deserializer::<SenderSt>()),
        Box::new(amaru_pure_stage::register_data_deserializer::<u8>()),
    ]
}

fn install_blocked_send(
    graph: &mut impl StageGraph,
) -> (StageRef<u8>, Sender<u8>, StageRef<u8>, Sender<u8>, Receiver<Note>) {
    let dest = graph.stage("dest", async |out: StageRef<Note>, msg: u8, eff| {
        match msg {
            0 => {
                eff.send(&out, Note::Saw(0)).await;
                eff.schedule_after(9u8, Duration::from_millis(10)).await;
                eff.wait(Duration::from_millis(80)).await;
            }
            other => eff.send(&out, Note::Saw(other)).await,
        }
        out
    });
    let dest_ref = dest.sender();
    let sender = graph.stage("sender", async |st: SenderSt, _msg: u8, eff| {
        eff.send(&st.dest, 2u8).await;
        eff.send(&st.out, Note::Sent(2)).await;
        st
    });
    let (out, rx) = graph.output::<Note>("out", 8);
    let sender_ref = sender.sender();
    graph.wire_up(dest, out.clone());
    graph.wire_up(sender, SenderSt { dest: dest_ref.clone(), out });
    let to_dest = graph.input(&dest_ref);
    let to_sender = graph.input(&sender_ref);
    (dest_ref, to_dest, sender_ref, to_sender, rx)
}

fn priority_wakeup_keeps_blocked_send(runtime: Runtime) {
    let _guards = register_notes();
    match runtime {
        Runtime::Simulation => {
            let mut network = SimulationBuilder::default().with_mailbox_size(1);
            let (dest, _to_dest, sender, _to_sender, mut out) = install_blocked_send(&mut network);
            let mut sim = network.run(test_runtime().handle());
            sim.enqueue_msg(&dest, [0]);
            sim.run(Run::default()).assert_sleeping();
            assert_eq!(out.try_next(), Some(Note::Saw(0)));
            sim.enqueue_msg(&dest, [1]);
            sim.enqueue_msg(&sender, [0]);
            sim.run(Run::skip_wakeups()).assert_idle();
            assert_eq!(out.drain().collect::<Vec<_>>(), vec![Note::Saw(9), Note::Saw(1), Note::Sent(2), Note::Saw(2)]);
        }
        Runtime::Tokio => {
            let rt = paused_runtime();
            let mut network = TokioBuilder::default().with_mailbox_size(1);
            let (_dest, to_dest, _sender, to_sender, mut out) = install_blocked_send(&mut network);
            let running = network.run(rt.handle().clone());
            rt.block_on(async move {
                to_dest.send(0).await.unwrap();
                settle().await;
                assert_eq!(out.try_next(), Some(Note::Saw(0)));
                to_dest.send(1).await.unwrap();
                to_sender.send(0).await.unwrap();
                tokio::time::sleep(Duration::from_millis(80)).await;
                settle().await;
                assert_eq!(
                    out.drain().collect::<Vec<_>>(),
                    vec![Note::Saw(9), Note::Saw(1), Note::Sent(2), Note::Saw(2)]
                );
            });
            running.abort();
        }
    }
}

fn install_cancel_after_due(graph: &mut impl StageGraph) -> (StageRef<u8>, Sender<u8>, Receiver<u8>) {
    let stage = graph.stage("sched", async |out: StageRef<u8>, msg: u8, eff| {
        if msg == 0 {
            let id = eff.schedule_after(7u8, Duration::from_millis(10)).await;
            eff.wait(Duration::from_millis(50)).await;
            let cancelled = eff.cancel_schedule(id).await;
            eff.send(&out, u8::from(cancelled)).await;
        } else {
            eff.send(&out, msg).await;
        }
        out
    });
    let (out, rx) = graph.output("out", 4);
    let stage_ref = stage.sender();
    graph.wire_up(stage, out);
    let tx = graph.input(&stage_ref);
    (stage_ref, tx, rx)
}

fn cancel_after_due_timer_keeps_message(runtime: Runtime) {
    let _guards = register();
    match runtime {
        Runtime::Simulation => {
            let mut network = SimulationBuilder::default();
            let (stage, _tx, mut out) = install_cancel_after_due(&mut network);
            let mut sim = network.run(test_runtime().handle());
            sim.enqueue_msg(&stage, [0]);
            sim.run(Run::skip_wakeups()).assert_idle();
            assert_eq!(out.drain().collect::<Vec<_>>(), vec![0, 7]);
        }
        Runtime::Tokio => {
            let rt = paused_runtime();
            let mut network = TokioBuilder::default();
            let (_stage, tx, mut out) = install_cancel_after_due(&mut network);
            let running = network.run(rt.handle().clone());
            rt.block_on(async move {
                tx.send(0).await.unwrap();
                tokio::time::sleep(Duration::from_millis(50)).await;
                settle().await;
                assert_eq!(out.drain().collect::<Vec<_>>(), vec![0, 7]);
            });
            running.abort();
        }
    }
}

fn install_budget(graph: &mut impl StageGraph) -> (StageRef<u8>, Sender<u8>, Receiver<u8>) {
    let stage = graph.stage("sched", async |out: StageRef<u8>, msg: u8, eff| {
        if msg == 0 {
            let id = eff.schedule_after(1u8, Duration::from_secs(30)).await;
            eff.schedule_after(2u8, Duration::from_secs(30)).await;
            if eff.cancel_schedule(id).await {
                eff.schedule_after(3u8, Duration::from_secs(30)).await;
                eff.send(&out, 1u8).await;
            }
        } else {
            eff.schedule_after(4u8, Duration::from_secs(30)).await;
            eff.send(&out, 2u8).await;
        }
        out
    });
    let (out, rx) = graph.output("out", 4);
    let stage_ref = stage.sender();
    graph.wire_up(stage, out);
    let tx = graph.input(&stage_ref);
    (stage_ref, tx, rx)
}

fn cancel_does_not_double_free_priority_slot(runtime: Runtime) {
    let _guards = register();
    match runtime {
        Runtime::Simulation => {
            let mut network = SimulationBuilder::default().with_priority_mailbox_size(2);
            let (stage, _tx, mut out) = install_budget(&mut network);
            let mut sim = network.run(test_runtime().handle());
            sim.enqueue_msg(&stage, [0]);
            sim.run(Run::default()).assert_sleeping();
            assert_eq!(out.try_next(), Some(1));
            sim.enqueue_msg(&stage, [1]);
            let panicked = std::panic::catch_unwind(AssertUnwindSafe(|| {
                sim.run(Run::default());
            }));
            assert!(panicked.is_err(), "the cancelled timer must not free a second slot");
            assert_eq!(out.try_next(), None);
        }
        Runtime::Tokio => {
            let rt = paused_runtime();
            let mut network = TokioBuilder::default().with_priority_mailbox_size(2);
            let (_stage, tx, mut out) = install_budget(&mut network);
            let running = network.run(rt.handle().clone());
            rt.block_on(async move {
                tx.send(0).await.unwrap();
                settle().await;
                assert_eq!(out.try_next(), Some(1));
                tx.send(1).await.unwrap();
                settle().await;
                assert_eq!(out.try_next(), None, "double budget release would accept another schedule");
                assert!(running.join().await.is_err(), "the extra schedule must exceed the priority cap");
            });
        }
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
enum ParentMsg {
    Boot,
    Occupy,
    Fill(u8),
    Gone,
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
enum Kick {
    Bind(StageRef<Mail>),
    Go,
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct ParentSt {
    out: StageRef<Out>,
    caller: StageRef<Kick>,
    callee: Option<StageRef<Mail>>,
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct BoundCaller {
    out: StageRef<Out>,
    callee: Option<StageRef<Mail>>,
}

struct SupervisedEnds {
    parent: StageRef<ParentMsg>,
    to_parent: Sender<ParentMsg>,
    caller: StageRef<Kick>,
    to_caller: Sender<Kick>,
    out: Receiver<Out>,
}

/// Callee is a supervised child. A root callee stops the simulation at `terminate` and, on
/// Tokio, aborts every statically wired stage, both before the call deadline.
fn install_supervised_terminating_call(graph: &mut impl StageGraph) -> SupervisedEnds {
    let caller = graph.stage("caller", async |mut st: BoundCaller, msg: Kick, eff| match msg {
        Kick::Bind(callee) => {
            st.callee = Some(callee);
            st
        }
        Kick::Go => {
            let callee = st.callee.clone().expect("callee bound");
            match eff.call(&callee, CALL_TIMEOUT, Mail::Ping).await {
                Some(value) => eff.send(&st.out, Out::Responded(value)).await,
                None => eff.send(&st.out, Out::Timeout).await,
            }
            st
        }
    });
    let caller_ref = caller.sender();
    let (out, rx) = graph.output("out", 8);
    graph.wire_up(caller, BoundCaller { out: out.clone(), callee: None });

    let parent = graph.stage("parent", async |mut st: ParentSt, msg: ParentMsg, eff| match msg {
        ParentMsg::Boot => {
            let callee = eff
                .stage("callee", async |out: StageRef<Out>, msg: Mail, eff| {
                    match msg {
                        Mail::Occupy => {
                            eff.send(&out, Out::Holding).await;
                            eff.wait(SLOT_WAIT).await;
                            return eff.terminate().await;
                        }
                        Mail::Filler(n) => eff.send(&out, Out::Saw(n)).await,
                        Mail::Ping(reply) => {
                            eff.send(&out, Out::Saw(9)).await;
                            eff.send(&reply, 7u32).await;
                        }
                    }
                    out
                })
                .await;
            let callee = eff.supervise(callee, ParentMsg::Gone);
            let callee = eff.wire_up(callee, st.out.clone()).await;
            eff.send(&st.caller, Kick::Bind(callee.clone())).await;
            st.callee = Some(callee);
            st
        }
        ParentMsg::Occupy => {
            let callee = st.callee.clone().expect("booted");
            eff.send(&callee, Mail::Occupy).await;
            st
        }
        ParentMsg::Fill(n) => {
            let callee = st.callee.clone().expect("booted");
            eff.send(&callee, Mail::Filler(n)).await;
            st
        }
        ParentMsg::Gone => st,
    });
    let parent_ref = parent.sender();
    graph.wire_up(parent, ParentSt { out, caller: caller_ref.clone(), callee: None });
    let to_parent = graph.input(&parent_ref);
    let to_caller = graph.input(&caller_ref);
    SupervisedEnds { parent: parent_ref, to_parent, caller: caller_ref, to_caller, out: rx }
}

fn callee_terminates_during_call(runtime: Runtime, queued: bool) {
    let _guards = register();
    let succeeded = |msg: &Out| matches!(msg, Out::Responded(_) | Out::Saw(9));
    let assert_timeout = |msgs: &[Out]| {
        assert!(msgs.contains(&Out::Timeout), "caller must time out after the callee is gone: {msgs:?}");
        assert!(msgs.iter().all(|msg| !succeeded(msg)), "{msgs:?}");
    };
    match runtime {
        Runtime::Simulation => {
            let mut network = SimulationBuilder::default().with_mailbox_size(1);
            let mut ends = install_supervised_terminating_call(&mut network);
            let mut sim = network.run(test_runtime().handle());
            sim.enqueue_msg(&ends.parent, [ParentMsg::Boot]);
            sim.run(Run::skip_wakeups()).assert_idle();
            sim.enqueue_msg(&ends.parent, [ParentMsg::Occupy]);
            sim.run(Run::default()).assert_sleeping();
            assert_eq!(ends.out.try_next(), Some(Out::Holding));
            if queued {
                sim.enqueue_msg(&ends.parent, [ParentMsg::Fill(1)]);
                sim.run(Run::default()).assert_sleeping();
            }
            sim.enqueue_msg(&ends.caller, [Kick::Go]);
            sim.run(Run::skip_wakeups()).assert_idle();
            assert_timeout(&ends.out.drain().collect::<Vec<_>>());
        }
        Runtime::Tokio => {
            let rt = paused_runtime();
            let mut network = TokioBuilder::default().with_mailbox_size(1);
            let mut ends = install_supervised_terminating_call(&mut network);
            let running = network.run(rt.handle().clone());
            rt.block_on(async move {
                ends.to_parent.send(ParentMsg::Boot).await.unwrap();
                settle().await;
                ends.to_parent.send(ParentMsg::Occupy).await.unwrap();
                settle().await;
                assert_eq!(ends.out.try_next(), Some(Out::Holding));
                if queued {
                    ends.to_parent.send(ParentMsg::Fill(1)).await.unwrap();
                    settle().await;
                }
                ends.to_caller.send(Kick::Go).await.unwrap();
                tokio::time::sleep(CALL_TIMEOUT).await;
                settle().await;
                assert_timeout(&ends.out.drain().collect::<Vec<_>>());
            });
            running.abort();
        }
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
enum Ask {
    Hold,
    Ping(u32, StageRef<u32>),
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
enum MultiOut {
    Holding,
    Saw(u32),
    Responded(u32),
    Timeout,
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct CallerN {
    id: u32,
    callee: StageRef<Ask>,
    out: StageRef<MultiOut>,
}

fn register_multi() -> amaru_pure_stage::DeserializerGuards {
    vec![
        Box::new(amaru_pure_stage::register_data_deserializer::<Ask>()),
        Box::new(amaru_pure_stage::register_data_deserializer::<MultiOut>()),
        Box::new(amaru_pure_stage::register_data_deserializer::<CallerN>()),
        Box::new(amaru_pure_stage::register_data_deserializer::<u8>()),
        Box::new(amaru_pure_stage::register_data_deserializer::<u32>()),
    ]
}

fn install_many(
    graph: &mut impl StageGraph,
) -> (StageRef<Ask>, Sender<Ask>, Vec<StageRef<u8>>, Vec<Sender<u8>>, Receiver<MultiOut>) {
    let callee = graph.stage("callee", async |out: StageRef<MultiOut>, msg: Ask, eff| {
        match msg {
            Ask::Hold => {
                eff.send(&out, MultiOut::Holding).await;
                eff.wait(SLOT_WAIT).await;
            }
            Ask::Ping(id, reply) => {
                eff.send(&out, MultiOut::Saw(id)).await;
                eff.send(&reply, id).await;
            }
        }
        out
    });
    let (out, rx) = graph.output::<MultiOut>("out", 16);
    let callee_ref = callee.sender();
    graph.wire_up(callee, out.clone());
    let mut refs = Vec::new();
    let mut txs = Vec::new();
    for (name, id) in [("c1", 1u32), ("c2", 2), ("c3", 3)] {
        let caller = graph.stage(name, async |st: CallerN, _msg: u8, eff| {
            match eff
                .call(&st.callee, LONG_CALL, {
                    let id = st.id;
                    move |reply| Ask::Ping(id, reply)
                })
                .await
            {
                Some(value) => eff.send(&st.out, MultiOut::Responded(value)).await,
                None => eff.send(&st.out, MultiOut::Timeout).await,
            }
            st
        });
        let caller_ref = caller.sender();
        graph.wire_up(caller, CallerN { id, callee: callee_ref.clone(), out: out.clone() });
        txs.push(graph.input(&caller_ref));
        refs.push(caller_ref);
    }
    let to_callee = graph.input(&callee_ref);
    (callee_ref, to_callee, refs, txs, rx)
}

fn many_callers_are_fifo(runtime: Runtime) {
    let _guards = register_multi();
    let saw_and_responded = |msgs: &[MultiOut]| {
        let saw: Vec<u32> = msgs
            .iter()
            .filter_map(|msg| match msg {
                MultiOut::Saw(n) => Some(*n),
                MultiOut::Holding | MultiOut::Responded(_) | MultiOut::Timeout => None,
            })
            .collect();
        let responded: Vec<u32> = msgs
            .iter()
            .filter_map(|msg| match msg {
                MultiOut::Responded(n) => Some(*n),
                MultiOut::Holding | MultiOut::Saw(_) | MultiOut::Timeout => None,
            })
            .collect();
        assert!(msgs.iter().all(|msg| !matches!(msg, MultiOut::Timeout)), "{msgs:?}");
        assert_eq!(saw, vec![1, 2, 3], "{msgs:?}");
        assert_eq!(responded, vec![1, 2, 3], "{msgs:?}");
    };
    match runtime {
        Runtime::Simulation => {
            let mut network = SimulationBuilder::default().with_mailbox_size(1);
            let (callee, _to_callee, refs, _txs, mut out) = install_many(&mut network);
            let mut sim = network.run(test_runtime().handle());
            sim.enqueue_msg(&callee, [Ask::Hold]);
            sim.run(Run::default()).assert_sleeping();
            assert_eq!(out.try_next(), Some(MultiOut::Holding));
            for caller in &refs {
                sim.enqueue_msg(caller, [0]);
            }
            sim.run(Run::skip_wakeups()).assert_idle();
            saw_and_responded(&out.drain().collect::<Vec<_>>());
        }
        Runtime::Tokio => {
            let rt = paused_runtime();
            let mut network = TokioBuilder::default().with_mailbox_size(1);
            let (_callee, to_callee, _refs, txs, mut out) = install_many(&mut network);
            let running = network.run(rt.handle().clone());
            rt.block_on(async move {
                to_callee.send(Ask::Hold).await.unwrap();
                settle().await;
                assert_eq!(out.try_next(), Some(MultiOut::Holding));
                for tx in txs {
                    tx.send(0).await.unwrap();
                }
                tokio::time::sleep(SLOT_WAIT).await;
                settle().await;
                saw_and_responded(&out.drain().collect::<Vec<_>>());
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

#[test]
fn call_delivers_when_slot_frees_simulation() {
    call_delivers_when_slot_frees(Runtime::Simulation);
}

#[test]
fn call_delivers_when_slot_frees_tokio() {
    call_delivers_when_slot_frees(Runtime::Tokio);
}

#[test]
fn priority_wakeup_keeps_blocked_send_simulation() {
    priority_wakeup_keeps_blocked_send(Runtime::Simulation);
}

#[test]
fn priority_wakeup_keeps_blocked_send_tokio() {
    priority_wakeup_keeps_blocked_send(Runtime::Tokio);
}

#[test]
fn cancel_after_due_timer_keeps_message_simulation() {
    cancel_after_due_timer_keeps_message(Runtime::Simulation);
}

#[test]
fn cancel_after_due_timer_keeps_message_tokio() {
    cancel_after_due_timer_keeps_message(Runtime::Tokio);
}

#[test]
fn cancel_does_not_double_free_priority_slot_simulation() {
    cancel_does_not_double_free_priority_slot(Runtime::Simulation);
}

#[test]
fn cancel_does_not_double_free_priority_slot_tokio() {
    cancel_does_not_double_free_priority_slot(Runtime::Tokio);
}

#[test]
fn callee_terminates_while_call_is_queued_simulation() {
    callee_terminates_during_call(Runtime::Simulation, true);
}

#[test]
fn callee_terminates_while_call_is_queued_tokio() {
    callee_terminates_during_call(Runtime::Tokio, true);
}

#[test]
fn callee_terminates_while_call_is_in_mailbox_simulation() {
    callee_terminates_during_call(Runtime::Simulation, false);
}

#[test]
fn callee_terminates_while_call_is_in_mailbox_tokio() {
    callee_terminates_during_call(Runtime::Tokio, false);
}

#[test]
fn many_callers_are_fifo_simulation() {
    many_callers_are_fifo(Runtime::Simulation);
}

#[test]
fn many_callers_are_fifo_tokio() {
    many_callers_are_fifo(Runtime::Tokio);
}

fn install_sibling_timeout(graph: &mut impl StageGraph) -> (StageRef<u8>, Sender<u8>, Receiver<u8>) {
    let stage = graph.stage("timeouts", async |out: StageRef<u8>, msg: u8, eff| {
        match msg {
            0 => {
                // Same deadline: only slot 0 is armed. Slot 1 stays stored until 0 is received.
                eff.set_timeout_at(0, Duration::from_millis(10), 1u8).await;
                eff.set_timeout_at(1, Duration::from_millis(10), 2u8).await;
                eff.wait(Duration::from_millis(40)).await;
                // Rearm is attempted here, while slot 0's message is still unreceived.
                eff.set_timeout_at(2, Duration::from_secs(30), 3u8).await;
                eff.wait(Duration::ZERO).await;
                eff.clear_timeout_at(1).await;
                eff.clear_timeout_at(2).await;
                eff.send(&out, 0u8).await;
            }
            other => eff.send(&out, other).await,
        }
        out
    });
    let (out, rx) = graph.output("out", 8);
    let stage_ref = stage.sender();
    graph.wire_up(stage, out);
    let tx = graph.input(&stage_ref);
    (stage_ref, tx, rx)
}

/// A later slot whose deadline has already passed is dropped by `clear_timeout_at` while an
/// earlier fired timeout is still waiting to be received.
fn cleared_sibling_timeout_is_not_delivered(runtime: Runtime) {
    let _guards = register();
    match runtime {
        Runtime::Simulation => {
            let mut network = SimulationBuilder::default();
            let (stage, _tx, mut out) = install_sibling_timeout(&mut network);
            let mut sim = network.run(test_runtime().handle());
            sim.enqueue_msg(&stage, [0]);
            sim.run(Run::skip_wakeups()).assert_idle();
            assert_eq!(out.drain().collect::<Vec<_>>(), vec![0, 1]);
        }
        Runtime::Tokio => {
            let rt = paused_runtime();
            let mut network = TokioBuilder::default();
            let (_stage, tx, mut out) = install_sibling_timeout(&mut network);
            let running = network.run(rt.handle().clone());
            rt.block_on(async move {
                tx.send(0).await.unwrap();
                tokio::time::sleep(Duration::from_millis(40)).await;
                settle().await;
                assert_eq!(out.drain().collect::<Vec<_>>(), vec![0, 1]);
            });
            running.abort();
        }
    }
}

#[test]
fn cleared_sibling_timeout_is_not_delivered_simulation() {
    cleared_sibling_timeout_is_not_delivered(Runtime::Simulation);
}

#[test]
fn cleared_sibling_timeout_is_not_delivered_tokio() {
    cleared_sibling_timeout_is_not_delivered(Runtime::Tokio);
}
