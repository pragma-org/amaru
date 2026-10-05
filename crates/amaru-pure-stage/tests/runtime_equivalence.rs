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
    CallAdmission, Receiver, Sender, StageGraph, StageRef, TrySend, assert_trace_contains,
    simulation::{Run, SimulationBuilder},
    tm_call, tm_resume_try_send, tm_try_send,
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

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
enum Report {
    Holding,
    Saw(u8),
    Try(u8, TrySend),
    /// `0` [`CallAdmission::NotAdmitted`], `1` [`CallAdmission::TimedOut`], `2` reply.
    Call(u8),
    Sent,
    Got(u8),
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct Probe {
    sink: StageRef<u8>,
    out: StageRef<Report>,
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct SelfSink {
    me: StageRef<u8>,
    out: StageRef<Report>,
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct Peer {
    id: u8,
    out: StageRef<Report>,
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct Fan {
    full: StageRef<u8>,
    other_a: StageRef<u8>,
    other_b: StageRef<u8>,
    out: StageRef<Report>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct ParkProbe {
    dest: StageRef<u8>,
    out: StageRef<Report>,
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
enum RootMsg {
    Boot,
    Kill,
    ChildGone,
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
enum DynParent {
    Boot,
    Kill,
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct RootState {
    callee: StageRef<Mail>,
    out: StageRef<Report>,
    parent: Option<StageRef<DynParent>>,
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct ParentState {
    callee: StageRef<Mail>,
    out: StageRef<Report>,
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct CallProbe {
    callee: StageRef<Mail>,
    out: StageRef<Report>,
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct CalleeSt {
    out: StageRef<Report>,
    slow: bool,
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

fn drain_report(rx: &mut Receiver<Report>) -> Vec<Report> {
    rx.drain().collect()
}

fn install_outcomes(
    graph: &mut impl StageGraph,
) -> (Sender<u8>, Sender<u8>, Receiver<Report>, StageRef<u8>, StageRef<u8>) {
    let sink = graph.stage_with_mailbox_size(
        "sink",
        async |out: StageRef<Report>, msg: u8, eff| {
            if msg == 0 {
                eff.send(&out, Report::Holding).await;
                eff.wait(HOLD).await;
            } else {
                eff.send(&out, Report::Saw(msg)).await;
            }
            out
        },
        1,
    );
    let sink_ref = sink.sender();
    let probe = graph.stage("probe", async |st: Probe, _: u8, eff| {
        let full = eff.try_send(&st.sink, 3u8).await;
        let blackhole = eff.try_send(&StageRef::blackhole(), 4u8).await;
        let gone = eff.try_send(&StageRef::named_for_tests("never-registered"), 5u8).await;
        eff.send(&st.out, Report::Try(3, full)).await;
        eff.send(&st.out, Report::Try(4, blackhole)).await;
        eff.send(&st.out, Report::Try(5, gone)).await;
        st
    });
    let (out, rx) = graph.output("out", 16);
    let probe_ref = probe.sender();
    graph.wire_up(sink, out.clone());
    graph.wire_up(probe, Probe { sink: sink_ref.clone(), out });
    let to_sink = graph.input(&sink_ref);
    let to_probe = graph.input(&probe_ref);
    (to_sink, to_probe, rx, sink_ref, probe_ref)
}

fn try_send_full_blackhole_and_gone(runtime: Runtime) {
    let _guards = register();
    match runtime {
        Runtime::Simulation => {
            let trace_buffer = TraceBuffer::new_shared(200, 1_000_000);
            let mut network = SimulationBuilder::default().with_trace_buffer(trace_buffer.clone());
            let (_to_sink, _to_probe, mut rx, sink_ref, probe_ref) = install_outcomes(&mut network);
            let sink_name = sink_ref.name().as_str().to_string();
            let probe_name = probe_ref.name().as_str().to_string();
            let mut sim = network.run(test_runtime().handle());
            sim.enqueue_msg(&sink_ref, [0]);
            sim.run(Run::default()).assert_sleeping();
            assert_eq!(rx.try_next(), Some(Report::Holding));
            sim.enqueue_msg(&sink_ref, [1]);
            assert_eq!(sim.mailbox_len(&sink_ref), 1);
            sim.enqueue_msg(&probe_ref, [0]);
            sim.run(Run::default()).assert_sleeping();
            assert_eq!(
                drain_report(&mut rx),
                vec![Report::Try(3, TrySend::Full), Report::Try(4, TrySend::Queued), Report::Try(5, TrySend::Gone),]
            );
            assert_eq!(sim.mailbox_len(&sink_ref), 1, "a full try_send must not enter the mailbox");
            let trace = trace_buffer.lock().hydrate_without_timestamps();
            assert_trace_contains(
                &sim,
                &[
                    tm_try_send(&probe_name, &sink_name, 3u8),
                    tm_try_send(&probe_name, "", 4u8),
                    tm_try_send(&probe_name, "never-registered", 5u8),
                ],
            );
            // Resumes are dropped by `assert_trace_contains`. The admission result lives there.
            let expected_resumes = [
                tm_resume_try_send(&probe_name, TrySend::Full),
                tm_resume_try_send(&probe_name, TrySend::Queued),
                tm_resume_try_send(&probe_name, TrySend::Gone),
            ];
            let mut found = 0;
            for entry in &trace {
                if found < expected_resumes.len() && expected_resumes[found] == *entry {
                    found += 1;
                }
            }
            assert_eq!(found, expected_resumes.len(), "try_send responses missing from the trace: {trace:?}");
            let mut network = SimulationBuilder::default();
            install_outcomes(&mut network);
            network.replay().run_trace(trace).expect("try_send trace replays");
        }
        Runtime::Tokio => {
            let rt = paused_runtime();
            let mut network = TokioBuilder::default();
            let (to_sink, to_probe, mut rx, _sink, _probe) = install_outcomes(&mut network);
            let running = network.run(rt.handle().clone());
            rt.block_on(async move {
                to_sink.send(0).await.unwrap();
                settle().await;
                assert_eq!(rx.try_next(), Some(Report::Holding));
                to_sink.send(1).await.unwrap();
                to_probe.send(0).await.unwrap();
                settle().await;
                assert_eq!(
                    drain_report(&mut rx),
                    vec![Report::Try(3, TrySend::Full), Report::Try(4, TrySend::Queued), Report::Try(5, TrySend::Gone),]
                );
            });
            running.abort();
        }
    }
}

fn install_self(graph: &mut impl StageGraph, size: usize) -> (StageRef<u8>, Sender<u8>, Receiver<Report>) {
    let stage = graph.stage_with_mailbox_size(
        "self",
        async |st: SelfSink, msg: u8, eff| {
            if msg == 0 {
                let first = eff.try_send(&st.me, 1u8).await;
                let second = eff.try_send(&st.me, 2u8).await;
                eff.send(&st.out, Report::Try(1, first)).await;
                eff.send(&st.out, Report::Try(2, second)).await;
            } else {
                eff.send(&st.out, Report::Saw(msg)).await;
            }
            st
        },
        size,
    );
    let me = stage.sender();
    let (out, rx) = graph.output("out", 8);
    graph.wire_up(stage, SelfSink { me: me.clone(), out });
    let tx = graph.input(&me);
    (me, tx, rx)
}

fn try_send_to_self(runtime: Runtime, size: usize) {
    let _guards = register();
    let expect = if size == 0 {
        vec![Report::Try(1, TrySend::Full), Report::Try(2, TrySend::Full)]
    } else {
        vec![Report::Try(1, TrySend::Queued), Report::Try(2, TrySend::Full), Report::Saw(1)]
    };
    match runtime {
        Runtime::Simulation => {
            let mut network = SimulationBuilder::default();
            let (me, _tx, mut rx) = install_self(&mut network, size);
            let mut sim = network.run(test_runtime().handle());
            sim.enqueue_msg(&me, [0]);
            sim.run(Run::skip_wakeups()).assert_idle();
            assert_eq!(drain_report(&mut rx), expect);
        }
        Runtime::Tokio => {
            let rt = paused_runtime();
            let mut network = TokioBuilder::default();
            let (_me, tx, mut rx) = install_self(&mut network, size);
            let running = network.run(rt.handle().clone());
            rt.block_on(async move {
                tx.send(0).await.unwrap();
                settle().await;
                assert_eq!(drain_report(&mut rx), expect);
            });
            running.abort();
        }
    }
}

struct Ends2 {
    full: StageRef<u8>,
    manager: StageRef<u8>,
    to_full: Sender<u8>,
    to_manager: Sender<u8>,
    out: Receiver<Report>,
}

fn install_fan(graph: &mut impl StageGraph) -> Ends2 {
    let full = graph.stage_with_mailbox_size(
        "full",
        async |st: Peer, msg: u8, eff| {
            if msg == 0 {
                eff.send(&st.out, Report::Holding).await;
                eff.wait(HOLD).await;
            } else {
                eff.send(&st.out, Report::Got(st.id)).await;
            }
            st
        },
        1,
    );
    let other_a = graph.stage("a", async |st: Peer, msg: u8, eff| {
        if msg != 0 {
            eff.send(&st.out, Report::Got(st.id)).await;
        }
        st
    });
    let other_b = graph.stage("b", async |st: Peer, msg: u8, eff| {
        if msg != 0 {
            eff.send(&st.out, Report::Got(st.id)).await;
        }
        st
    });
    let full_ref = full.sender();
    let a_ref = other_a.sender();
    let b_ref = other_b.sender();
    let manager = graph.stage("manager", async |st: Fan, _: u8, eff| {
        for (id, peer) in [(0u8, &st.full), (1, &st.other_a), (2, &st.other_b)] {
            let outcome = eff.try_send(peer, 5u8).await;
            eff.send(&st.out, Report::Try(id, outcome)).await;
        }
        st
    });
    let (out, rx) = graph.output("out", 16);
    let manager_ref = manager.sender();
    graph.wire_up(full, Peer { id: 0, out: out.clone() });
    graph.wire_up(other_a, Peer { id: 1, out: out.clone() });
    graph.wire_up(other_b, Peer { id: 2, out: out.clone() });
    graph.wire_up(manager, Fan { full: full_ref.clone(), other_a: a_ref, other_b: b_ref, out });
    let to_full = graph.input(&full_ref);
    let to_manager = graph.input(&manager_ref);
    Ends2 { full: full_ref, manager: manager_ref, to_full, to_manager, out: rx }
}

fn try_send_fanout_skips_full_peer(runtime: Runtime) {
    let _guards = register();
    match runtime {
        Runtime::Simulation => {
            let mut network = SimulationBuilder::default();
            let mut ends = install_fan(&mut network);
            let mut sim = network.run(test_runtime().handle());
            sim.enqueue_msg(&ends.full, [0]);
            sim.run(Run::default()).assert_sleeping();
            assert_eq!(ends.out.try_next(), Some(Report::Holding));
            sim.enqueue_msg(&ends.full, [1]);
            assert_eq!(sim.mailbox_len(&ends.full), 1);
            sim.enqueue_msg(&ends.manager, [0]);
            sim.run(Run::default()).assert_sleeping();
            assert_fan(&drain_report(&mut ends.out));
        }
        Runtime::Tokio => {
            let rt = paused_runtime();
            let mut network = TokioBuilder::default();
            let mut ends = install_fan(&mut network);
            let running = network.run(rt.handle().clone());
            rt.block_on(async move {
                ends.to_full.send(0).await.unwrap();
                settle().await;
                assert_eq!(ends.out.try_next(), Some(Report::Holding));
                ends.to_full.send(1).await.unwrap();
                ends.to_manager.send(0).await.unwrap();
                settle().await;
                assert_fan(&drain_report(&mut ends.out));
            });
            running.abort();
        }
    }
}

fn assert_fan(msgs: &[Report]) {
    assert!(msgs.contains(&Report::Try(0, TrySend::Full)), "{msgs:?}");
    assert!(msgs.contains(&Report::Try(1, TrySend::Queued)), "{msgs:?}");
    assert!(msgs.contains(&Report::Try(2, TrySend::Queued)), "{msgs:?}");
    assert!(msgs.contains(&Report::Got(1)) && msgs.contains(&Report::Got(2)), "{msgs:?}");
    assert!(!msgs.contains(&Report::Got(0)), "{msgs:?}");
}

fn install_park(
    graph: &mut impl StageGraph,
) -> (StageRef<u8>, StageRef<u8>, StageRef<u8>, Sender<u8>, Sender<u8>, Sender<u8>, Receiver<Report>) {
    let dest = graph.stage_with_mailbox_size(
        "dest",
        async |out: StageRef<Report>, msg: u8, eff| {
            if msg == 0 {
                eff.send(&out, Report::Holding).await;
                eff.wait(HOLD).await;
            } else {
                eff.send(&out, Report::Saw(msg)).await;
            }
            out
        },
        0,
    );
    let dest_ref = dest.sender();
    let blocker = graph.stage("blocker", async |st: ParkProbe, _: u8, eff| {
        eff.send(&st.dest, 7u8).await;
        eff.send(&st.out, Report::Sent).await;
        st
    });
    let probe = graph.stage("probe", async |st: ParkProbe, _: u8, eff| {
        let outcome = eff.try_send(&st.dest, 9u8).await;
        eff.send(&st.out, Report::Try(9, outcome)).await;
        st
    });
    let (out, rx) = graph.output("out", 8);
    let blocker_ref = blocker.sender();
    let probe_ref = probe.sender();
    graph.wire_up(dest, out.clone());
    let st = ParkProbe { dest: dest_ref.clone(), out };
    graph.wire_up(blocker, st.clone());
    graph.wire_up(probe, st);
    (
        dest_ref.clone(),
        blocker_ref.clone(),
        probe_ref.clone(),
        graph.input(&dest_ref),
        graph.input(&blocker_ref),
        graph.input(&probe_ref),
        rx,
    )
}

fn try_send_does_not_pass_a_parked_sender(runtime: Runtime) {
    let _guards = register();
    match runtime {
        Runtime::Simulation => {
            let mut network = SimulationBuilder::default();
            let (dest, blocker, probe, _d, _b, _p, mut rx) = install_park(&mut network);
            let mut sim = network.run(test_runtime().handle());
            let started = sim.now();
            sim.enqueue_msg(&dest, [0]);
            sim.run(Run::default()).assert_sleeping();
            assert_eq!(rx.try_next(), Some(Report::Holding));
            sim.enqueue_msg(&blocker, [0]);
            sim.enqueue_msg(&probe, [0]);
            sim.run(Run::until(started + CALL_TIMEOUT)).assert_sleeping();
            assert_eq!(drain_report(&mut rx), vec![Report::Try(9, TrySend::Full)]);
            sim.run(Run::until(started + HOLD));
            let rest = drain_report(&mut rx);
            assert!(rest.contains(&Report::Saw(7)) && rest.contains(&Report::Sent), "{rest:?}");
            assert!(!rest.contains(&Report::Saw(9)), "{rest:?}");
        }
        Runtime::Tokio => {
            let rt = paused_runtime();
            let mut network = TokioBuilder::default();
            let (_d, _b, _p, to_dest, to_blocker, to_probe, mut rx) = install_park(&mut network);
            let running = network.run(rt.handle().clone());
            rt.block_on(async move {
                to_dest.send(0).await.unwrap();
                settle().await;
                assert_eq!(rx.try_next(), Some(Report::Holding));
                to_blocker.send(0).await.unwrap();
                settle().await;
                to_probe.send(0).await.unwrap();
                settle().await;
                assert_eq!(rx.try_next(), Some(Report::Try(9, TrySend::Full)));
                assert_eq!(rx.try_next(), None, "parked send must not finish while the receiver is busy");
                tokio::time::sleep(HOLD).await;
                settle().await;
                let rest = drain_report(&mut rx);
                assert!(rest.contains(&Report::Saw(7)) && rest.contains(&Report::Sent), "{rest:?}");
                assert!(!rest.contains(&Report::Saw(9)), "{rest:?}");
            });
            running.abort();
        }
    }
}

fn install_capacity(
    graph: &mut impl StageGraph,
    size: Option<usize>,
) -> (StageRef<u8>, StageRef<u8>, Sender<u8>, Sender<u8>, Receiver<Report>) {
    let sink = match size {
        Some(size) => graph.stage_with_mailbox_size(
            "sink",
            async |out: StageRef<Report>, msg: u8, eff| {
                if msg == 0 {
                    eff.send(&out, Report::Holding).await;
                    eff.wait(HOLD).await;
                } else {
                    eff.send(&out, Report::Saw(msg)).await;
                }
                out
            },
            size,
        ),
        None => graph.stage("sink", async |out: StageRef<Report>, msg: u8, eff| {
            if msg == 0 {
                eff.send(&out, Report::Holding).await;
                eff.wait(HOLD).await;
            } else {
                eff.send(&out, Report::Saw(msg)).await;
            }
            out
        }),
    };
    let sink_ref = sink.sender();
    let probe = graph.stage("probe", async |st: Probe, _: u8, eff| {
        let outcome = eff.try_send(&st.sink, 9u8).await;
        eff.send(&st.out, Report::Try(9, outcome)).await;
        st
    });
    let (out, rx) = graph.output("out", 8);
    let probe_ref = probe.sender();
    graph.wire_up(sink, out.clone());
    graph.wire_up(probe, Probe { sink: sink_ref.clone(), out });
    (sink_ref.clone(), probe_ref.clone(), graph.input(&sink_ref), graph.input(&probe_ref), rx)
}

fn mailbox_accepts_n_then_try_send_is_full(runtime: Runtime, size: Option<usize>) {
    let _guards = register();
    let n = size.unwrap_or(amaru_pure_stage::DEFAULT_MAILBOX_SIZE);
    match runtime {
        Runtime::Simulation => {
            let mut network = SimulationBuilder::default();
            let (sink, probe, _s, _p, mut rx) = install_capacity(&mut network, size);
            let mut sim = network.run(test_runtime().handle());
            sim.enqueue_msg(&sink, [0]);
            sim.run(Run::default()).assert_sleeping();
            assert_eq!(rx.try_next(), Some(Report::Holding));
            sim.enqueue_msg(&sink, std::iter::repeat_n(1u8, n));
            assert_eq!(sim.mailbox_len(&sink), n);
            sim.enqueue_msg(&probe, [0]);
            sim.run(Run::default()).assert_sleeping();
            assert_eq!(drain_report(&mut rx), vec![Report::Try(9, TrySend::Full)]);
            assert_eq!(sim.mailbox_len(&sink), n);
        }
        Runtime::Tokio => {
            let rt = paused_runtime();
            let mut network = TokioBuilder::default();
            let (_sink, _probe, to_sink, to_probe, mut rx) = install_capacity(&mut network, size);
            let running = network.run(rt.handle().clone());
            rt.block_on(async move {
                to_sink.send(0).await.unwrap();
                settle().await;
                assert_eq!(rx.try_next(), Some(Report::Holding));
                for _ in 0..n {
                    to_sink.send(1).await.unwrap();
                }
                to_probe.send(0).await.unwrap();
                settle().await;
                assert_eq!(drain_report(&mut rx), vec![Report::Try(9, TrySend::Full)]);
            });
            running.abort();
        }
    }
}

fn install_blocking(
    graph: &mut impl StageGraph,
) -> (StageRef<u8>, StageRef<u8>, Sender<u8>, Sender<u8>, Receiver<Report>) {
    let sink = graph.stage_with_mailbox_size(
        "sink",
        async |out: StageRef<Report>, msg: u8, eff| {
            if msg == 0 {
                eff.send(&out, Report::Holding).await;
                eff.wait(HOLD).await;
            } else {
                eff.send(&out, Report::Saw(msg)).await;
            }
            out
        },
        1,
    );
    let sink_ref = sink.sender();
    let sender = graph.stage("sender", async |st: ParkProbe, _: u8, eff| {
        eff.send(&st.dest, 7u8).await;
        eff.send(&st.out, Report::Sent).await;
        st
    });
    let (out, rx) = graph.output("out", 8);
    let sender_ref = sender.sender();
    graph.wire_up(sink, out.clone());
    graph.wire_up(sender, ParkProbe { dest: sink_ref.clone(), out });
    (sink_ref.clone(), sender_ref.clone(), graph.input(&sink_ref), graph.input(&sender_ref), rx)
}

fn blocking_send_parks_until_the_mailbox_drains(runtime: Runtime) {
    let _guards = register();
    match runtime {
        Runtime::Simulation => {
            let mut network = SimulationBuilder::default();
            let (sink, sender, _s, _t, mut rx) = install_blocking(&mut network);
            let mut sim = network.run(test_runtime().handle());
            let started = sim.now();
            sim.enqueue_msg(&sink, [0]);
            sim.run(Run::default()).assert_sleeping();
            assert_eq!(rx.try_next(), Some(Report::Holding));
            sim.enqueue_msg(&sink, [1]);
            sim.enqueue_msg(&sender, [0]);
            sim.run(Run::until(started + CALL_TIMEOUT)).assert_sleeping();
            assert_eq!(drain_report(&mut rx), Vec::<Report>::new(), "blocking send must still be parked");
            sim.run(Run::until(started + HOLD));
            let rest = drain_report(&mut rx);
            assert!(
                rest.contains(&Report::Saw(1)) && rest.contains(&Report::Saw(7)) && rest.contains(&Report::Sent),
                "{rest:?}"
            );
        }
        Runtime::Tokio => {
            let rt = paused_runtime();
            let mut network = TokioBuilder::default();
            let (_sink, _sender, to_sink, to_sender, mut rx) = install_blocking(&mut network);
            let running = network.run(rt.handle().clone());
            rt.block_on(async move {
                to_sink.send(0).await.unwrap();
                settle().await;
                assert_eq!(rx.try_next(), Some(Report::Holding));
                to_sink.send(1).await.unwrap();
                to_sender.send(0).await.unwrap();
                settle().await;
                assert_eq!(rx.try_next(), None, "blocking send must still be parked");
                tokio::time::sleep(HOLD).await;
                settle().await;
                let rest = drain_report(&mut rx);
                assert!(
                    rest.contains(&Report::Saw(1)) && rest.contains(&Report::Saw(7)) && rest.contains(&Report::Sent),
                    "{rest:?}"
                );
            });
            running.abort();
        }
    }
}

fn call_code(result: CallAdmission<u32>) -> u8 {
    match result {
        CallAdmission::NotAdmitted(_) => 0,
        CallAdmission::TimedOut(_) => 1,
        CallAdmission::Reply(_) => 2,
    }
}

fn install_call_probe(
    graph: &mut impl StageGraph,
    mailbox: usize,
    slow_reply: bool,
) -> (StageRef<Mail>, StageRef<u8>, Sender<Mail>, Sender<u8>, Receiver<Report>) {
    let callee = graph.stage_with_mailbox_size(
        "callee",
        async |st: CalleeSt, msg: Mail, eff| {
            match msg {
                Mail::Occupy => {
                    eff.send(&st.out, Report::Holding).await;
                    eff.wait(HOLD).await;
                }
                Mail::Filler(n) => eff.send(&st.out, Report::Saw(n)).await,
                Mail::Ping(reply) => {
                    if st.slow {
                        eff.wait(HOLD).await;
                    }
                    eff.send(&reply, 1u32).await;
                    eff.send(&st.out, Report::Saw(9)).await;
                }
            }
            st
        },
        mailbox,
    );
    let callee_ref = callee.sender();
    let caller = graph.stage("caller", async |st: CallProbe, _: u8, eff| {
        let result = eff.call_with_admission(&st.callee, CALL_TIMEOUT, Mail::Ping).await;
        eff.send(&st.out, Report::Call(call_code(result))).await;
        st
    });
    let (out, rx) = graph.output("out", 8);
    let caller_ref = caller.sender();
    graph.wire_up(callee, CalleeSt { out: out.clone(), slow: slow_reply });
    graph.wire_up(caller, CallProbe { callee: callee_ref.clone(), out });
    (callee_ref.clone(), caller_ref.clone(), graph.input(&callee_ref), graph.input(&caller_ref), rx)
}

fn call_cancelled_before_admission(runtime: Runtime) {
    let _guards = register();
    match runtime {
        Runtime::Simulation => {
            let mut network = SimulationBuilder::default();
            let (callee, caller, _c, _k, mut rx) = install_call_probe(&mut network, 1, false);
            let mut sim = network.run(test_runtime().handle());
            let started = sim.now();
            sim.enqueue_msg(&callee, [Mail::Occupy]);
            sim.run(Run::default()).assert_sleeping();
            assert_eq!(rx.try_next(), Some(Report::Holding));
            sim.enqueue_msg(&callee, [Mail::Filler(1)]);
            sim.enqueue_msg(&caller, [0]);
            sim.run(Run::until(started + CALL_TIMEOUT));
            assert_eq!(drain_report(&mut rx), vec![Report::Call(0)]);
            assert_eq!(sim.mailbox_len(&callee), 1);
            sim.run(Run::until(started + HOLD));
            let rest = drain_report(&mut rx);
            assert_eq!(rest, vec![Report::Saw(1)], "cancelled call must not be delivered: {rest:?}");
        }
        Runtime::Tokio => {
            let rt = paused_runtime();
            let mut network = TokioBuilder::default();
            let (_callee, _caller, to_callee, to_caller, mut rx) = install_call_probe(&mut network, 1, false);
            let running = network.run(rt.handle().clone());
            rt.block_on(async move {
                to_callee.send(Mail::Occupy).await.unwrap();
                settle().await;
                assert_eq!(rx.try_next(), Some(Report::Holding));
                to_callee.send(Mail::Filler(1)).await.unwrap();
                to_caller.send(0).await.unwrap();
                settle().await;
                tokio::time::sleep(CALL_TIMEOUT).await;
                settle().await;
                assert_eq!(rx.try_next(), Some(Report::Call(0)));
                tokio::time::sleep(HOLD).await;
                settle().await;
                assert_eq!(drain_report(&mut rx), vec![Report::Saw(1)]);
            });
            running.abort();
        }
    }
}

fn call_timed_out_is_not_retracted(runtime: Runtime) {
    let _guards = register();
    match runtime {
        Runtime::Simulation => {
            let mut network = SimulationBuilder::default();
            let (_callee, caller, _c, _k, mut rx) = install_call_probe(&mut network, 4, true);
            let mut sim = network.run(test_runtime().handle());
            let started = sim.now();
            sim.enqueue_msg(&caller, [0]);
            sim.run(Run::until(started + CALL_TIMEOUT));
            assert_eq!(drain_report(&mut rx), vec![Report::Call(1)], "timed out after admission");
            sim.run(Run::until(started + HOLD));
            let rest = drain_report(&mut rx);
            assert!(rest.contains(&Report::Saw(9)), "timed-out request stays queued: {rest:?}");
            assert!(!rest.iter().any(|msg| matches!(msg, Report::Call(_))), "{rest:?}");
        }
        Runtime::Tokio => {
            let rt = paused_runtime();
            let mut network = TokioBuilder::default();
            let (_callee, _caller, _to_callee, to_caller, mut rx) = install_call_probe(&mut network, 4, true);
            let running = network.run(rt.handle().clone());
            rt.block_on(async move {
                to_caller.send(0).await.unwrap();
                settle().await;
                tokio::time::sleep(CALL_TIMEOUT).await;
                settle().await;
                assert_eq!(rx.try_next(), Some(Report::Call(1)));
                assert_eq!(rx.try_next(), None, "the callee is still inside its wait");
                tokio::time::sleep(HOLD).await;
                settle().await;
                assert_eq!(drain_report(&mut rx), vec![Report::Saw(9)]);
            });
            running.abort();
        }
    }
}

fn install_dynamic(graph: &mut impl StageGraph) -> (Sender<u8>, Receiver<Report>) {
    let root = graph.stage("root", async |out: StageRef<Report>, _: u8, eff| {
        let child = eff
            .stage_with_mailbox_size(
                "child",
                async |out: StageRef<Report>, msg: u8, eff| {
                    if msg == 0 {
                        eff.send(&out, Report::Holding).await;
                        eff.wait(HOLD).await;
                    } else {
                        eff.send(&out, Report::Saw(msg)).await;
                    }
                    out
                },
                1,
            )
            .await;
        let child = eff.wire_up(child, out.clone()).await;
        eff.send(&child, 0u8).await;
        eff.wait(SLOT_WAIT).await;
        let first = eff.try_send(&child, 1u8).await;
        let second = eff.try_send(&child, 2u8).await;
        eff.send(&out, Report::Try(1, first)).await;
        eff.send(&out, Report::Try(2, second)).await;
        out
    });
    let root_ref = root.sender();
    let (out, rx) = graph.output("out", 8);
    graph.wire_up(root, out);
    let tx = graph.input(&root_ref);
    (tx, rx)
}

fn dynamic_stage_mailbox_size(runtime: Runtime) {
    let _guards = register();
    let expect = vec![Report::Holding, Report::Try(1, TrySend::Queued), Report::Try(2, TrySend::Full)];
    match runtime {
        Runtime::Simulation => {
            let mut network = SimulationBuilder::default();
            let (root_tx, mut rx) = {
                let root = network.stage("root", async |out: StageRef<Report>, _: u8, eff| {
                    let child = eff
                        .stage_with_mailbox_size(
                            "child",
                            async |out: StageRef<Report>, msg: u8, eff| {
                                if msg == 0 {
                                    eff.send(&out, Report::Holding).await;
                                    eff.wait(HOLD).await;
                                } else {
                                    eff.send(&out, Report::Saw(msg)).await;
                                }
                                out
                            },
                            1,
                        )
                        .await;
                    let child = eff.wire_up(child, out.clone()).await;
                    eff.send(&child, 0u8).await;
                    eff.wait(SLOT_WAIT).await;
                    let first = eff.try_send(&child, 1u8).await;
                    let second = eff.try_send(&child, 2u8).await;
                    eff.send(&out, Report::Try(1, first)).await;
                    eff.send(&out, Report::Try(2, second)).await;
                    out
                });
                let root_ref = root.sender();
                let (out, rx) = network.output("out", 8);
                network.wire_up(root, out);
                (root_ref, rx)
            };
            let mut sim = network.run(test_runtime().handle());
            let started = sim.now();
            sim.enqueue_msg(&root_tx, [0]);
            sim.run(Run::until(started + SLOT_WAIT));
            assert_eq!(drain_report(&mut rx), expect);
        }
        Runtime::Tokio => {
            let rt = paused_runtime();
            let mut network = TokioBuilder::default();
            let (tx, mut rx) = install_dynamic(&mut network);
            let running = network.run(rt.handle().clone());
            rt.block_on(async move {
                tx.send(0).await.unwrap();
                settle().await;
                tokio::time::sleep(SLOT_WAIT).await;
                settle().await;
                assert_eq!(drain_report(&mut rx), expect);
            });
            running.abort();
        }
    }
}

fn install_caller_gone(
    graph: &mut impl StageGraph,
) -> (StageRef<Mail>, StageRef<RootMsg>, Sender<Mail>, Sender<RootMsg>, Receiver<Report>) {
    let callee = graph.stage_with_mailbox_size(
        "callee",
        async |out: StageRef<Report>, msg: Mail, eff| {
            match msg {
                Mail::Occupy => {
                    eff.send(&out, Report::Holding).await;
                    eff.wait(HOLD).await;
                }
                Mail::Filler(n) => eff.send(&out, Report::Saw(n)).await,
                Mail::Ping(_) => eff.send(&out, Report::Saw(9)).await,
            }
            out
        },
        1,
    );
    let callee_ref = callee.sender();
    let root = graph.stage("root", async |mut st: RootState, msg: RootMsg, eff| {
        match msg {
            RootMsg::Boot => {
                let parent = eff
                    .stage("parent", async |st: ParentState, msg: DynParent, eff| {
                        match msg {
                            DynParent::Boot => {
                                let caller = eff
                                    .stage("caller", async |st: ParentState, _: u8, eff| {
                                        let result = eff.call_with_admission(&st.callee, LONG_CALL, Mail::Ping).await;
                                        eff.send(&st.out, Report::Call(call_code(result))).await;
                                        st
                                    })
                                    .await;
                                let caller = eff
                                    .wire_up(caller, ParentState { callee: st.callee.clone(), out: st.out.clone() })
                                    .await;
                                eff.send(&caller, 0u8).await;
                            }
                            DynParent::Kill => return eff.terminate().await,
                        }
                        st
                    })
                    .await;
                let parent = eff.supervise(parent, RootMsg::ChildGone);
                let parent = eff.wire_up(parent, ParentState { callee: st.callee.clone(), out: st.out.clone() }).await;
                eff.send(&parent, DynParent::Boot).await;
                st.parent = Some(parent);
            }
            RootMsg::Kill => {
                let parent = st.parent.clone().expect("parent");
                eff.send(&parent, DynParent::Kill).await;
            }
            RootMsg::ChildGone => {}
        }
        st
    });
    let root_ref = root.sender();
    let (out, rx) = graph.output("out", 8);
    graph.wire_up(callee, out.clone());
    graph.wire_up(root, RootState { callee: callee_ref.clone(), out, parent: None });
    (callee_ref.clone(), root_ref.clone(), graph.input(&callee_ref), graph.input(&root_ref), rx)
}

fn caller_gone_while_call_is_queued(runtime: Runtime) {
    let _guards = register();
    match runtime {
        Runtime::Simulation => {
            let mut network = SimulationBuilder::default();
            let (callee, root, _c, _r, mut rx) = install_caller_gone(&mut network);
            let mut sim = network.run(test_runtime().handle());
            let started = sim.now();
            sim.enqueue_msg(&callee, [Mail::Occupy]);
            sim.run(Run::default()).assert_sleeping();
            assert_eq!(rx.try_next(), Some(Report::Holding));
            sim.enqueue_msg(&callee, [Mail::Filler(1)]);
            sim.enqueue_msg(&root, [RootMsg::Boot]);
            sim.run(Run::default()).assert_sleeping();
            assert_eq!(rx.try_next(), None, "call is only queued");
            sim.enqueue_msg(&root, [RootMsg::Kill]);
            sim.run(Run::until(started + HOLD));
            let rest = drain_report(&mut rx);
            assert!(rest.contains(&Report::Saw(1)), "{rest:?}");
            assert!(!rest.contains(&Report::Saw(9)), "queued call survived the caller: {rest:?}");
            assert!(!rest.iter().any(|msg| matches!(msg, Report::Call(_))), "{rest:?}");
        }
        Runtime::Tokio => {
            let rt = paused_runtime();
            let mut network = TokioBuilder::default();
            let (_callee, _root, to_callee, to_root, mut rx) = install_caller_gone(&mut network);
            let running = network.run(rt.handle().clone());
            rt.block_on(async move {
                to_callee.send(Mail::Occupy).await.unwrap();
                settle().await;
                assert_eq!(rx.try_next(), Some(Report::Holding));
                to_callee.send(Mail::Filler(1)).await.unwrap();
                to_root.send(RootMsg::Boot).await.unwrap();
                settle().await;
                assert_eq!(rx.try_next(), None);
                to_root.send(RootMsg::Kill).await.unwrap();
                settle().await;
                tokio::time::sleep(HOLD).await;
                settle().await;
                let rest = drain_report(&mut rx);
                assert!(rest.contains(&Report::Saw(1)), "{rest:?}");
                assert!(!rest.contains(&Report::Saw(9)), "{rest:?}");
                assert!(!rest.iter().any(|msg| matches!(msg, Report::Call(_))), "{rest:?}");
            });
            running.abort();
        }
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct ParkCaller {
    out: StageRef<Report>,
    callee: Option<StageRef<Mail>>,
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct ParkParent {
    out: StageRef<Report>,
    caller: StageRef<Kick>,
    callee: Option<StageRef<Mail>>,
}

/// Callee mailbox holds one message. Occupy is in flight, one filler is queued, and the call
/// is only parked. The callee then terminates. The request never entered the mailbox.
fn install_parked_call_loses_callee(
    graph: &mut impl StageGraph,
) -> (StageRef<ParentMsg>, StageRef<Kick>, Sender<ParentMsg>, Sender<Kick>, Receiver<Report>) {
    let caller = graph.stage("caller", async |mut st: ParkCaller, msg: Kick, eff| match msg {
        Kick::Bind(callee) => {
            st.callee = Some(callee);
            st
        }
        Kick::Go => {
            let callee = st.callee.clone().expect("callee bound");
            let result = eff.call_with_admission(&callee, CALL_TIMEOUT, Mail::Ping).await;
            eff.send(&st.out, Report::Call(call_code(result))).await;
            st
        }
    });
    let caller_ref = caller.sender();
    let (out, rx) = graph.output("out", 8);
    graph.wire_up(caller, ParkCaller { out: out.clone(), callee: None });

    let parent = graph.stage("parent", async |mut st: ParkParent, msg: ParentMsg, eff| match msg {
        ParentMsg::Boot => {
            let callee = eff
                .stage_with_mailbox_size(
                    "callee",
                    async |out: StageRef<Report>, msg: Mail, eff| {
                        match msg {
                            Mail::Occupy => {
                                eff.send(&out, Report::Holding).await;
                                eff.wait(SLOT_WAIT).await;
                                return eff.terminate().await;
                            }
                            Mail::Filler(n) => eff.send(&out, Report::Saw(n)).await,
                            Mail::Ping(_) => eff.send(&out, Report::Saw(9)).await,
                        }
                        out
                    },
                    1,
                )
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
    graph.wire_up(parent, ParkParent { out, caller: caller_ref.clone(), callee: None });
    (parent_ref.clone(), caller_ref.clone(), graph.input(&parent_ref), graph.input(&caller_ref), rx)
}

fn callee_gone_while_call_is_only_parked(runtime: Runtime) {
    let _guards = register();
    let assert_not_admitted = |msgs: &[Report]| {
        assert!(msgs.contains(&Report::Call(0)), "parked call must be NotAdmitted: {msgs:?}");
        assert!(!msgs.contains(&Report::Saw(9)), "parked call must not be delivered: {msgs:?}");
        assert!(!msgs.contains(&Report::Call(1)), "parked call must not be TimedOut: {msgs:?}");
    };
    match runtime {
        Runtime::Simulation => {
            let mut network = SimulationBuilder::default();
            let (parent, caller, _to_parent, _to_caller, mut rx) = install_parked_call_loses_callee(&mut network);
            let mut sim = network.run(test_runtime().handle());
            sim.enqueue_msg(&parent, [ParentMsg::Boot]);
            sim.run(Run::skip_wakeups()).assert_idle();
            sim.enqueue_msg(&parent, [ParentMsg::Occupy]);
            sim.run(Run::default()).assert_sleeping();
            assert_eq!(rx.try_next(), Some(Report::Holding));
            sim.enqueue_msg(&parent, [ParentMsg::Fill(1)]);
            sim.run(Run::default()).assert_sleeping();
            sim.enqueue_msg(&caller, [Kick::Go]);
            sim.run(Run::skip_wakeups()).assert_idle();
            assert_not_admitted(&drain_report(&mut rx));
        }
        Runtime::Tokio => {
            let rt = paused_runtime();
            let mut network = TokioBuilder::default();
            let (_parent, _caller, to_parent, to_caller, mut rx) = install_parked_call_loses_callee(&mut network);
            let running = network.run(rt.handle().clone());
            rt.block_on(async move {
                to_parent.send(ParentMsg::Boot).await.unwrap();
                settle().await;
                to_parent.send(ParentMsg::Occupy).await.unwrap();
                settle().await;
                assert_eq!(rx.try_next(), Some(Report::Holding));
                to_parent.send(ParentMsg::Fill(1)).await.unwrap();
                settle().await;
                to_caller.send(Kick::Go).await.unwrap();
                tokio::time::sleep(CALL_TIMEOUT).await;
                settle().await;
                assert_not_admitted(&drain_report(&mut rx));
            });
            running.abort();
        }
    }
}

#[test]
fn try_send_full_blackhole_and_gone_simulation() {
    try_send_full_blackhole_and_gone(Runtime::Simulation);
}

#[test]
fn try_send_full_blackhole_and_gone_tokio() {
    try_send_full_blackhole_and_gone(Runtime::Tokio);
}

#[test]
fn try_send_to_self_size_one_simulation() {
    try_send_to_self(Runtime::Simulation, 1);
}

#[test]
fn try_send_to_self_size_one_tokio() {
    try_send_to_self(Runtime::Tokio, 1);
}

#[test]
fn try_send_to_self_size_zero_simulation() {
    try_send_to_self(Runtime::Simulation, 0);
}

#[test]
fn try_send_to_self_size_zero_tokio() {
    try_send_to_self(Runtime::Tokio, 0);
}

#[test]
fn try_send_fanout_skips_full_peer_simulation() {
    try_send_fanout_skips_full_peer(Runtime::Simulation);
}

#[test]
fn try_send_fanout_skips_full_peer_tokio() {
    try_send_fanout_skips_full_peer(Runtime::Tokio);
}

#[test]
fn try_send_does_not_pass_a_parked_sender_simulation() {
    try_send_does_not_pass_a_parked_sender(Runtime::Simulation);
}

#[test]
fn try_send_does_not_pass_a_parked_sender_tokio() {
    try_send_does_not_pass_a_parked_sender(Runtime::Tokio);
}

#[test]
fn default_mailbox_is_10_simulation() {
    mailbox_accepts_n_then_try_send_is_full(Runtime::Simulation, None);
}

#[test]
fn default_mailbox_is_10_tokio() {
    mailbox_accepts_n_then_try_send_is_full(Runtime::Tokio, None);
}

#[test]
fn per_stage_mailbox_size_simulation() {
    mailbox_accepts_n_then_try_send_is_full(Runtime::Simulation, Some(2));
}

#[test]
fn per_stage_mailbox_size_tokio() {
    mailbox_accepts_n_then_try_send_is_full(Runtime::Tokio, Some(2));
}

#[test]
fn blocking_send_parks_until_the_mailbox_drains_simulation() {
    blocking_send_parks_until_the_mailbox_drains(Runtime::Simulation);
}

#[test]
fn blocking_send_parks_until_the_mailbox_drains_tokio() {
    blocking_send_parks_until_the_mailbox_drains(Runtime::Tokio);
}

#[test]
fn call_cancelled_before_admission_simulation() {
    call_cancelled_before_admission(Runtime::Simulation);
}

#[test]
fn call_cancelled_before_admission_tokio() {
    call_cancelled_before_admission(Runtime::Tokio);
}

#[test]
fn call_timed_out_is_not_retracted_simulation() {
    call_timed_out_is_not_retracted(Runtime::Simulation);
}

#[test]
fn call_timed_out_is_not_retracted_tokio() {
    call_timed_out_is_not_retracted(Runtime::Tokio);
}

#[test]
fn dynamic_stage_mailbox_size_simulation() {
    dynamic_stage_mailbox_size(Runtime::Simulation);
}

#[test]
fn dynamic_stage_mailbox_size_tokio() {
    dynamic_stage_mailbox_size(Runtime::Tokio);
}

#[test]
fn caller_gone_while_call_is_queued_simulation() {
    caller_gone_while_call_is_queued(Runtime::Simulation);
}

#[test]
fn caller_gone_while_call_is_queued_tokio() {
    caller_gone_while_call_is_queued(Runtime::Tokio);
}

#[test]
fn callee_gone_while_call_is_only_parked_simulation() {
    callee_gone_while_call_is_only_parked(Runtime::Simulation);
}

#[test]
fn callee_gone_while_call_is_only_parked_tokio() {
    callee_gone_while_call_is_only_parked(Runtime::Tokio);
}
