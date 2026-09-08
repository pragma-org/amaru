#![expect(clippy::bool_assert_comparison)]
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

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, Waker},
    time::Duration,
};

use amaru_pure_stage::{
    BoxFuture, DurationDist, Effect, ExternalEffectAPI, Instant, OrTerminateWith, OutputEffect, PRIORITY_MAILBOX_SIZE,
    Receiver, Resources, ScheduleId, SendData, StageGraph, StageGraphRunning, StageRef, assert_effect_match,
    assert_trace_contains,
    simulation::{RandStdRng, Run, SimulationBuilder, running::OverrideResult},
    tm_add_stage, tm_call, tm_external_effect, tm_send, tm_wire_stage,
    trace_buffer::{TraceBuffer, TraceEntry},
};
use rand::{SeedableRng, rngs::StdRng};
use tokio::runtime::Builder;
use tracing_subscriber::EnvFilter;

#[expect(clippy::expect_used)]
fn test_runtime() -> &'static tokio::runtime::Runtime {
    static RT: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RT.get_or_init(|| tokio::runtime::Runtime::new().expect("tokio runtime"))
}

/// World-owned park: `run()` never completes; the test injects `()` via `complete_external`.
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct Park;

impl ExternalEffectAPI for Park {
    type Response = ();
    const SIMULATED_DURATION: DurationDist = DurationDist::UntilResolved;

    fn run(self: Box<Self>, _resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap(|_| std::future::pending())
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct State(u32, StageRef<u32>);

#[test]
fn basic() {
    let mut network = SimulationBuilder::default();
    let basic = network.stage("basic", async |mut state: State, msg: u32, eff| {
        state.0 += msg;
        eff.send(&state.1, state.0).await;
        state
    });
    let (output, mut rx) = network.output("output", 10);
    let basic = network.wire_up(basic, State(1u32, output.clone()));
    let mut running = network.run(test_runtime().handle());

    running.run(Run::default()).assert_idle();

    running.enqueue_msg(&basic, [1]);
    running.run(Run::skip_and_resolve()).assert_idle();
    assert_eq!(rx.drain().collect::<Vec<_>>(), vec![2]);
}

#[test]
fn automatic() {
    let trace_buffer = TraceBuffer::new_shared(100, 1_000_000);
    let std_rng = StdRng::from_seed([0; 32]);
    let mut network =
        SimulationBuilder::default().with_trace_buffer(trace_buffer.clone()).with_eval_strategy(RandStdRng(std_rng));

    fn basic(network: &mut impl StageGraph) -> (StageRef<u32>, Receiver<u32>, StageRef<u32>) {
        let basic = network.stage("basic", async |mut state: State, msg: u32, eff| {
            state.0 += msg;
            eff.wait(Duration::from_secs(10)).await;
            eff.send(&state.1, state.0).await;
            state
        });
        let (output, rx) = network.output("output", 10);
        let basic = network.wire_up(basic, State(1u32, output.clone()));
        (basic.without_state(), rx, output)
    }

    let (in_ref, mut rx, output) = basic(&mut network);
    let mut running = network.run(test_runtime().handle());

    running.enqueue_msg(&in_ref, [1, 2, 3]);
    running.run(Run::skip_and_resolve()).assert_idle();
    assert_eq!(rx.drain().collect::<Vec<_>>(), vec![2, 4, 7]);

    let trace = trace_buffer.lock().hydrate_without_timestamps();

    const EXPECTED: &[&str] = &[
        "State { stage: Name(\"basic-1\"), state: SendDataValue { typetag: \"simulation::State\", value: Array([Integer(1), Map([(Text(\"name\"), Text(\"output-2\"))])]) } }",
        "State { stage: Name(\"output-2\"), state: SendDataValue { typetag: \"amaru_pure_stage::types::MpscSender<u32>\", value: Map([]) } }",
        "Input { stage: Name(\"basic-1\"), input: SendDataValue { typetag: \"u32\", value: Integer(1) } }",
        "Resume { stage: Name(\"basic-1\"), response: Unit }",
        "Suspend(Wait { at_stage: Name(\"basic-1\"), duration: 10s })",
        "Clock(Instant(10s))",
        "Resume { stage: Name(\"basic-1\"), response: WaitResponse(Instant(10s)) }",
        "Suspend(Send { from: Name(\"basic-1\"), to: Name(\"output-2\"), msg: SendDataValue { typetag: \"u32\", value: Integer(2) } })",
        "Input { stage: Name(\"output-2\"), input: SendDataValue { typetag: \"u32\", value: Integer(2) } }",
        "Resume { stage: Name(\"output-2\"), response: Unit }",
        "Suspend(External { at_stage: Name(\"output-2\"), effect: UnknownExternalEffect { value: SendDataValue { typetag: \"amaru_pure_stage::output::OutputEffect<u32>\", value: Map([(Text(\"name\"), Text(\"output-2\")), (Text(\"msg\"), Integer(2)), (Text(\"sender\"), Map([]))]) } } })",
        "Resume { stage: Name(\"output-2\"), response: ExternalResponse(SendDataValue { typetag: \"()\", value: Array([]) }) }",
        "State { stage: Name(\"output-2\"), state: SendDataValue { typetag: \"amaru_pure_stage::types::MpscSender<u32>\", value: Map([]) } }",
        "Resume { stage: Name(\"basic-1\"), response: Unit }",
        "State { stage: Name(\"basic-1\"), state: SendDataValue { typetag: \"simulation::State\", value: Array([Integer(2), Map([(Text(\"name\"), Text(\"output-2\"))])]) } }",
        "Input { stage: Name(\"basic-1\"), input: SendDataValue { typetag: \"u32\", value: Integer(2) } }",
        "Resume { stage: Name(\"basic-1\"), response: Unit }",
        "Suspend(Wait { at_stage: Name(\"basic-1\"), duration: 10s })",
        "Clock(Instant(20s))",
        "Resume { stage: Name(\"basic-1\"), response: WaitResponse(Instant(20s)) }",
        "Suspend(Send { from: Name(\"basic-1\"), to: Name(\"output-2\"), msg: SendDataValue { typetag: \"u32\", value: Integer(4) } })",
        "Input { stage: Name(\"output-2\"), input: SendDataValue { typetag: \"u32\", value: Integer(4) } }",
        "Resume { stage: Name(\"output-2\"), response: Unit }",
        "Suspend(External { at_stage: Name(\"output-2\"), effect: UnknownExternalEffect { value: SendDataValue { typetag: \"amaru_pure_stage::output::OutputEffect<u32>\", value: Map([(Text(\"name\"), Text(\"output-2\")), (Text(\"msg\"), Integer(4)), (Text(\"sender\"), Map([]))]) } } })",
        "Resume { stage: Name(\"basic-1\"), response: Unit }",
        "State { stage: Name(\"basic-1\"), state: SendDataValue { typetag: \"simulation::State\", value: Array([Integer(4), Map([(Text(\"name\"), Text(\"output-2\"))])]) } }",
        "Input { stage: Name(\"basic-1\"), input: SendDataValue { typetag: \"u32\", value: Integer(3) } }",
        "Resume { stage: Name(\"basic-1\"), response: Unit }",
        "Suspend(Wait { at_stage: Name(\"basic-1\"), duration: 10s })",
        "Resume { stage: Name(\"output-2\"), response: ExternalResponse(SendDataValue { typetag: \"()\", value: Array([]) }) }",
        "State { stage: Name(\"output-2\"), state: SendDataValue { typetag: \"amaru_pure_stage::types::MpscSender<u32>\", value: Map([]) } }",
        "Clock(Instant(30s))",
        "Resume { stage: Name(\"basic-1\"), response: WaitResponse(Instant(30s)) }",
        "Suspend(Send { from: Name(\"basic-1\"), to: Name(\"output-2\"), msg: SendDataValue { typetag: \"u32\", value: Integer(7) } })",
        "Input { stage: Name(\"output-2\"), input: SendDataValue { typetag: \"u32\", value: Integer(7) } }",
        "Resume { stage: Name(\"output-2\"), response: Unit }",
        "Suspend(External { at_stage: Name(\"output-2\"), effect: UnknownExternalEffect { value: SendDataValue { typetag: \"amaru_pure_stage::output::OutputEffect<u32>\", value: Map([(Text(\"name\"), Text(\"output-2\")), (Text(\"msg\"), Integer(7)), (Text(\"sender\"), Map([]))]) } } })",
        "Resume { stage: Name(\"basic-1\"), response: Unit }",
        "State { stage: Name(\"basic-1\"), state: SendDataValue { typetag: \"simulation::State\", value: Array([Integer(7), Map([(Text(\"name\"), Text(\"output-2\"))])]) } }",
        "Resume { stage: Name(\"output-2\"), response: ExternalResponse(SendDataValue { typetag: \"()\", value: Array([]) }) }",
        "State { stage: Name(\"output-2\"), state: SendDataValue { typetag: \"amaru_pure_stage::types::MpscSender<u32>\", value: Map([]) } }",
    ];

    pretty_assertions::assert_eq!(trace.iter().map(|t| format!("{t:?}")).collect::<Vec<_>>(), EXPECTED);

    let mut network = SimulationBuilder::default();
    basic(&mut network);
    let mut replay = network.replay();
    replay.run_trace(trace).unwrap();

    assert_eq!(replay.latest_state(in_ref.name()), Some(&State(7, output.clone()) as &dyn SendData));
    assert_eq!(replay.is_running(in_ref.name()), false);
    assert_eq!(replay.is_idle(in_ref.name()), true);
    assert_eq!(replay.is_terminating(output.name()), false);
    assert_eq!(replay.is_idle(output.name()), true);
    assert_eq!(replay.clock(), Instant::at_offset(Duration::from_secs(30), Duration::ZERO));
}

#[test]
fn breakpoint() {
    let _guard = amaru_pure_stage::register_data_deserializer::<u32>();
    let trace_buffer = TraceBuffer::new_shared(100, 1_000_000);
    let std_rng = StdRng::from_seed([0; 32]);
    let mut network =
        SimulationBuilder::default().with_trace_buffer(trace_buffer).with_eval_strategy(RandStdRng(std_rng));
    let basic = network.stage("basic", async |mut state: State, msg: u32, eff| {
        state.0 += msg;
        eff.send(&state.1, state.0).await;
        state
    });
    let (output, mut rx) = network.output("output", 10);
    let basic = network.wire_up(basic, State(1u32, output.clone()));
    let mut running = network.run(test_runtime().handle());

    running.enqueue_msg(&basic, [1, 2, 3]);
    let basic_name = basic.name().clone();
    let output_name = output.name().clone();
    running.breakpoint("send4", {
        let basic_name = basic_name.clone();
        let output_name = output_name.clone();
        move |eff| {
            matches!(
                eff,
                Effect::Send { from, to, msg, .. }
                    if from == &basic_name &&
                        to == &output_name &&
                        *msg == Box::new(4u32) as Box<dyn SendData>
            )
        }
    });
    running.run(Run::skip_wakeups()).assert_breakpoint("send4");
    {
        let hit = running.breakpoint_effect();
        assert_effect_match(hit.effect(), tm_send(basic_name.as_str(), output_name.as_str(), 4u32));
    }
    assert_trace_contains(&running, &[tm_send(basic_name.as_str(), output_name.as_str(), 4u32)]);
    running.run(Run::skip_and_resolve()).assert_idle();
    assert_eq!(rx.drain().collect::<Vec<_>>(), vec![2, 4, 7]);
}

#[test]
fn overrides() {
    let _guard = amaru_pure_stage::register_data_deserializer::<State>();
    let _guard = amaru_pure_stage::register_data_deserializer::<u32>();
    let _guard = amaru_pure_stage::register_effect_deserializer::<OutputEffect<u32>>();

    tracing_subscriber::fmt().with_test_writer().with_env_filter(EnvFilter::from_default_env()).try_init().ok();

    let trace_buffer = TraceBuffer::new_shared(100, 1_000_000);
    let guard = TraceBuffer::drop_guard(&trace_buffer);
    let mut network = SimulationBuilder::default().with_trace_buffer(trace_buffer);
    let basic = network.stage("basic", async |mut state: State, msg: u32, eff| {
        state.0 += msg;
        eff.send(&state.1, state.0).await;
        state
    });
    let (output, mut rx) = network.output("output", 10);
    let basic = network.wire_up(basic, State(1u32, output.clone()));
    let mut running = network.run(test_runtime().handle());

    let count = Arc::new(AtomicUsize::new(0));
    let count2 = count.clone();
    running.enqueue_msg(&basic, [1, 2, 3]);
    running.override_external_effect(1, move |eff: Box<OutputEffect<u32>>| {
        if eff.msg > 2 {
            count2.fetch_add(1, Ordering::Relaxed);
            OverrideResult::handled(())
        } else {
            OverrideResult::no_match(eff)
        }
    });
    running.run(Run::skip_and_resolve()).assert_idle();
    assert_eq!(rx.drain().collect::<Vec<_>>(), vec![2, 7]);
    assert_eq!(count.load(Ordering::Relaxed), 1);

    guard.defuse();
}

#[test]
fn backpressure() {
    tracing_subscriber::fmt().with_test_writer().with_env_filter(EnvFilter::from_default_env()).try_init().ok();

    let mut network = SimulationBuilder::default().with_mailbox_size(1);

    let sender = network.stage("sender", async |target, msg: u32, eff| {
        eff.send(&target, msg).await;
        target
    });

    let pressure = network.stage("pressure", async |mut state, msg: u32, eff| {
        state += msg;
        // Park so further sends fill the mailbox (size 1) and then block.
        let () = eff.external(Park).await;
        state
    });

    let sender = network.wire_up(sender, pressure.sender());
    let pressure = network.wire_up(pressure, 1u32);

    let mut running = network.run(test_runtime().handle());

    running.enqueue_msg(&sender, [1]);
    running.run(Run::default()).assert_busy([pressure.name()]);

    running.enqueue_msg(&sender, [2]);
    running.run(Run::default()).assert_busy([pressure.name()]);

    running.enqueue_msg(&sender, [3]);
    running.run(Run::default()).assert_busy([pressure.name()]);
    assert_eq!(running.mailbox_len(&pressure), 1);
    assert_eq!(running.get_state(&sender), None, "sender must be blocked on Send");

    running.complete_external(pressure.name(), ());
    running.run(Run::default()).assert_busy([pressure.name()]);
    running.complete_external(pressure.name(), ());
    running.run(Run::default()).assert_busy([pressure.name()]);
    running.complete_external(pressure.name(), ());
    running.run(Run::default()).assert_idle();
    assert_eq!(*running.get_state(&pressure).unwrap(), 7);
}

/// Self-scheduled messages are delivered via the priority path even when the bulk mailbox is full,
/// and are preferred over bulk messages when both are pending.
#[test]
fn schedule_delivered_despite_full_bulk_mailbox() {
    tracing_subscriber::fmt().with_test_writer().with_env_filter(EnvFilter::from_default_env()).try_init().ok();

    let mut network = SimulationBuilder::default().with_mailbox_size(1);

    let stage = network.stage("stage", async |mut state: Vec<u32>, msg: u32, eff| {
        if msg == 0 {
            // Schedule control message immediately, then suspend so bulk can fill the mailbox.
            eff.schedule_after(99, Duration::ZERO).await;
            eff.clock().await;
        }
        state.push(msg);
        state
    });
    let stage = network.wire_up(stage, Vec::<u32>::new());
    let mut running = network.run(test_runtime().handle());

    running.enqueue_msg(&stage, [0u32]);
    running.breakpoint("clock", {
        let stage = stage.clone();
        move |eff| matches!(eff, Effect::Clock { at_stage } if at_stage == stage.name())
    });

    running.run(Run::skip_wakeups()).assert_breakpoint("clock");

    // Fill bulk mailbox while stage is suspended on clock.
    running.enqueue_msg(&stage, [1u32]);
    assert_eq!(running.mailbox_len(&stage), 1);

    running.clear_breakpoint("clock");
    running.run(Run::skip_wakeups()).assert_idle();

    let state = running.get_state(&stage).unwrap();
    assert!(state.contains(&99), "scheduled control message must be delivered: {state:?}");
    assert!(state.contains(&1), "bulk message must still be delivered: {state:?}");
    // Control (priority) before remaining bulk.
    let pos_ctrl = state.iter().position(|m| *m == 99).unwrap();
    let pos_bulk = state.iter().position(|m| *m == 1).unwrap();
    assert!(pos_ctrl < pos_bulk, "priority message must precede bulk: {state:?}");
}

#[test]
fn schedule_cap_panics_at_limit_plus_one() {
    tracing_subscriber::fmt().with_test_writer().with_env_filter(EnvFilter::from_default_env()).try_init().ok();

    // Use an explicit limit (not the default) so the test documents configurability.
    const LIMIT: usize = 3;
    let mut network = SimulationBuilder::default().with_priority_mailbox_size(LIMIT);
    let stage = network.stage("stage", async |state: (), msg: u32, eff| {
        if msg == 0 {
            for i in 0..=LIMIT as u32 {
                // Future schedules so they stay outstanding without being received.
                eff.schedule_after(i + 1, Duration::from_secs(10)).await;
            }
        }
        state
    });
    let stage = network.wire_up(stage, ());
    let mut running = network.run(test_runtime().handle());

    running.enqueue_msg(&stage, [0u32]);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        running.run(Run::skip_wakeups());
    }));
    assert!(result.is_err(), "expected panic when exceeding configured priority mailbox size");
}

#[test]
fn cancel_schedule_frees_priority_slot() {
    tracing_subscriber::fmt().with_test_writer().with_env_filter(EnvFilter::from_default_env()).try_init().ok();

    let mut network = SimulationBuilder::default().with_priority_mailbox_size(PRIORITY_MAILBOX_SIZE);
    let stage = network.stage("stage", async |state: Option<ScheduleId>, msg: u32, eff| {
        match msg {
            0 => {
                let mut last = None;
                for i in 0..PRIORITY_MAILBOX_SIZE as u32 {
                    // Far-future so they stay armed until we cancel (do not auto-advance into them).
                    last = Some(eff.schedule_after(100 + i, Duration::from_secs(10)).await);
                }
                last
            }
            1 => {
                let id = state.expect("id from previous message");
                assert!(eff.cancel_schedule(id).await);
                // Slot freed: one more schedule must succeed without exceeding the cap.
                eff.schedule_after(200, Duration::from_secs(10)).await;
                None
            }
            _ => state,
        }
    });
    let stage = network.wire_up(stage, None);
    let mut running = network.run(test_runtime().handle());

    running.enqueue_msg(&stage, [0u32]);
    // Stop at Sleeping so far-future schedules are not delivered yet.
    running.run(Run::default()).assert_sleeping();
    assert!(running.get_state(&stage).unwrap().is_some());

    running.enqueue_msg(&stage, [1u32]);
    running.run(Run::default()).assert_sleeping();
    // No panic and state cleared after successful cancel+reschedule.
    assert!(running.get_state(&stage).unwrap().is_none());
}

#[test]
fn set_timeout_replaces_and_clear_timeout_cancels() {
    let mut network = SimulationBuilder::default();
    let stage = network.stage("stage", async |state: Vec<u32>, msg: u32, eff| match msg {
        0 => {
            eff.set_timeout(Duration::from_secs(10), 1).await;
            eff.set_timeout(Duration::from_secs(10), 2).await;
            state
        }
        3 => {
            eff.clear_timeout().await;
            state
        }
        n => {
            let mut state = state;
            state.push(n);
            state
        }
    });
    let stage = network.wire_up(stage, Vec::new());
    let rt = Builder::new_multi_thread().enable_all().build().unwrap();
    let mut running = network.run(rt.handle());

    running.enqueue_msg(&stage, [0u32]);
    running.run(Run::default()).assert_sleeping();
    running.enqueue_msg(&stage, [3u32]);
    running.run(Run::skip_wakeups()).assert_idle();
    assert!(running.get_state(&stage).unwrap().is_empty());
    assert!(!running.skip_to_next_wakeup(None));
}

#[test]
fn many_timeout_slots_do_not_overflow_priority_mailbox() {
    let mut network = SimulationBuilder::default().with_priority_mailbox_size(PRIORITY_MAILBOX_SIZE);
    let stage = network.stage("stage", async |state: (), msg: u32, eff| {
        if msg == 0 {
            for i in 0..300u32 {
                eff.set_timeout_at(i as u64, Duration::from_secs(60), 1000 + i).await;
            }
        }
        state
    });
    let stage = network.wire_up(stage, ());
    let mut running = network.run(test_runtime().handle());
    running.enqueue_msg(&stage, [0u32]);
    running.run(Run::default()).assert_sleeping();
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
enum State2 {
    Empty,
    Full(u32, Instant, Instant),
}

#[test]
fn clock() {
    let mut network = SimulationBuilder::default();
    let basic = network.stage("basic", async |_state: State2, msg: u32, eff| {
        let now = eff.clock().await;
        let later = eff.wait(Duration::from_secs(1)).await;
        State2::Full(msg, now, later)
    });
    let basic = network.wire_up(basic, State2::Empty);
    let mut running = network.run(test_runtime().handle());

    running.enqueue_msg(&basic, [42]);
    let now = running.now();
    running.run(Run::skip_wakeups()).assert_idle();
    let later = running.now();
    assert_eq!(running.get_state(&basic).unwrap(), &State2::Full(42u32, now, later));
    assert_eq!(later.checked_since(now).unwrap(), Duration::from_secs(1));

    running.enqueue_msg(&basic, [43]);
    let wakeup = running.run(Run::until(later + Duration::from_millis(100))).assert_sleeping();
    assert_eq!(wakeup, later + Duration::from_secs(1));
}

#[test]
fn clock_manual() {
    let mut network = SimulationBuilder::default();
    let stage = network.stage("basic", async |_state, msg: u32, eff| {
        let now = eff.clock().await;
        let later = eff.wait(Duration::from_secs(1)).await;
        State2::Full(msg, now, later)
    });
    let stage = network.wire_up(stage, State2::Empty);
    let mut running = network.run(test_runtime().handle());

    running.enqueue_msg(&stage, [42]);
    let now = running.now();
    running.run(Run::default()).assert_sleeping();
    assert_eq!(running.get_state(&stage), None);

    let intermediate = running.now() + Duration::from_millis(100);
    let target = intermediate + Duration::from_millis(900);

    assert!(!running.skip_to_next_wakeup(Some(intermediate)));
    assert_eq!(running.now(), intermediate);

    assert!(running.skip_to_next_wakeup(None));
    assert_eq!(running.now(), target);

    running.run(Run::default()).assert_idle();
    let later = running.now();

    assert_eq!(running.get_state(&stage).unwrap(), &State2::Full(42u32, now, later));
    assert_eq!(later.checked_since(now).unwrap(), Duration::from_secs(1));

    assert!(!running.skip_to_next_wakeup(Some(later + Duration::from_secs(1))));
    assert_eq!(running.now(), later + Duration::from_secs(1));
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct State3(u32, StageRef<Msg3>);

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct Msg3(u32, StageRef<u32>);

#[test]
fn call() {
    tracing_subscriber::fmt().with_test_writer().with_env_filter(EnvFilter::from_default_env()).try_init().ok();

    let _guard = amaru_pure_stage::register_data_deserializer::<Msg3>();
    let trace_buffer = TraceBuffer::new_shared(1, 1000000);
    let guard = TraceBuffer::drop_guard(&trace_buffer);

    let mut network = SimulationBuilder::default().with_trace_buffer(trace_buffer);
    let caller = network.stage("caller", async |mut state: State3, msg: u32, eff| {
        state.0 = eff
            .call(&state.1, Duration::from_secs(2), move |cr| Msg3(msg + 1, cr))
            .or_terminate_with(&eff, async |_| ())
            .await;
        state
    });

    let callee = network.stage("callee", async |state, msg: Msg3, eff| {
        eff.wait(Duration::from_secs(1)).await;
        eff.send(&msg.1, msg.0 * 2).await;
        state
    });
    let caller = network.wire_up(caller, State3(1u32, callee.sender()));
    let callee = network.wire_up(callee, ());

    let mut sim = network.run(test_runtime().handle());

    sim.enqueue_msg(&caller, [1]);
    sim.run(Run::skip_wakeups()).assert_idle();
    assert_eq!(sim.get_state(&caller).unwrap().0, 4);

    sim.enqueue_msg(&caller, [2]);
    sim.breakpoint("call", {
        let caller = caller.clone();
        move |eff| matches!(eff, Effect::Call { from, .. } if from == caller.name())
    });
    sim.run(Run::skip_wakeups()).assert_breakpoint("call");
    {
        let hit = sim.breakpoint_effect();
        assert_effect_match(
            hit.effect(),
            tm_call(caller.name().as_str(), callee.name().as_str(), Duration::from_secs(2)),
        );
    }
    sim.clear_breakpoint("call");
    sim.run(Run::skip_wakeups()).assert_idle();
    assert_eq!(sim.get_state(&caller).unwrap().0, 6);

    guard.defuse();
}

#[test]
fn call_external_sender_in_simulation() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = amaru_pure_stage::register_data_deserializer::<Msg3>();

    let mut network = SimulationBuilder::default();
    let callee = network.stage("callee", async |state, msg: Msg3, eff| {
        eff.send(&msg.1, msg.0 * 2).await;
        state
    });
    let callee = network.wire_up(callee, ());
    let sender = network.input(callee.clone().without_state());

    let mut sim = network.run(test_runtime().handle());
    let call = rt.spawn(async move { sender.call::<u32>(|cr| Msg3(3, cr), Duration::from_secs(1)).await });

    rt.block_on(sim.await_external_input());
    sim.run(Run::skip_wakeups()).assert_idle();

    assert_eq!(rt.block_on(call).unwrap().unwrap(), 6);
}

#[test]
fn call_timeout_terminates_graph() {
    let _guard = amaru_pure_stage::register_data_deserializer::<Msg3>();
    let trace_buffer = TraceBuffer::new_shared(1, 1000000);
    let guard = TraceBuffer::drop_guard(&trace_buffer);
    let mut network = SimulationBuilder::default().with_trace_buffer(trace_buffer);

    // caller times out quickly; callee sleeps longer -> triggers terminate
    let caller = network.stage("caller", async |state: State3, msg: u32, eff| {
        eff.call(&state.1, Duration::from_millis(10), move |cr| Msg3(msg + 1, cr))
            // Returning terminate here should trigger graph termination
            // (SimulationRunning.termination should complete)
            .or_terminate_with(&eff, async |_| {})
            .await;
        state
    });

    let callee = network.stage("callee", async |state, _msg: Msg3, eff| {
        eff.wait(Duration::from_secs(1)).await; // Ensure we exceed caller timeout
        state
    });

    let caller = network.wire_up(caller, State3(0u32, callee.sender()));
    network.wire_up(callee, ());

    let mut sim = network.run(test_runtime().handle());

    sim.enqueue_msg(&caller, [1]);
    // Run until blocked, then assert termination flips true
    let mut term = sim.termination();
    assert_eq!(term.as_mut().poll(&mut Context::from_waker(Waker::noop())), Poll::Pending);

    sim.run(Run::skip_wakeups()).assert_terminated(caller.name()); // drive effects

    assert!(sim.is_terminated(), "simulation should report terminated");
    assert_eq!(term.as_mut().poll(&mut Context::from_waker(Waker::noop())), Poll::Ready(()));

    guard.defuse();
}

#[test]
fn create_stage_within_stage() {
    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct ParentState {
        child_ref: Option<StageRef<u32>>,
        output: StageRef<u32>,
    }

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct ChildState {
        value: u32,
        output: StageRef<u32>,
    }

    let _guard = amaru_pure_stage::register_data_deserializer::<u32>();
    let _guard = amaru_pure_stage::register_effect_deserializer::<OutputEffect<u32>>();
    let trace_buffer = TraceBuffer::new_shared(100, 1_000_000);
    let mut network = SimulationBuilder::default().with_trace_buffer(trace_buffer);

    // Parent stage that creates a child stage
    let parent = network.stage("parent", async |mut state: ParentState, msg: u32, eff| {
        if state.child_ref.is_none() {
            // Create a child stage within the parent stage
            let child = eff
                .stage("child", async |mut state: ChildState, msg: u32, eff| {
                    state.value += msg;
                    eff.send(&state.output, state.value).await;
                    state
                })
                .await;

            // Wire up the child stage with initial state that includes the output reference
            let child_ref = eff.wire_up(child, ChildState { value: 0u32, output: state.output.clone() }).await;
            state.child_ref = Some(child_ref);
        }

        // Send a message to the child stage
        if let Some(ref child) = state.child_ref {
            eff.send(child, msg).await;
        }

        state
    });

    let (output, mut rx) = network.output("output", 10);
    let parent = network.wire_up(parent, ParentState { child_ref: None, output: output.clone() });
    let mut running = network.run(test_runtime().handle());

    running.run(Run::default()).assert_idle();
    running.enqueue_msg(&parent, [42]);
    running.run(Run::skip_and_resolve()).assert_idle();

    assert_eq!(rx.drain().collect::<Vec<_>>(), vec![42]);

    let child = running.get_state(&parent).unwrap().child_ref.clone().expect("child was wired");
    assert_trace_contains(
        &running,
        &[
            tm_add_stage(parent.name(), "child"),
            tm_wire_stage(parent.name().as_str(), "child"),
            tm_send(parent.name().as_str(), child.name().as_str(), 42u32),
            tm_send(child.name().as_str(), output.name().as_str(), 42u32),
            tm_external_effect::<OutputEffect<u32>>(output.name()),
        ],
    );
}

/// Test that `use_virtual_child_stages(true)` allows a parent stage to successfully execute
/// `eff.stage(...)` + `eff.wire_up(...)` (effects are recorded, parent receives the StageRef
/// and can send to it), while preventing the child stage from actually being materialized
/// in the simulation. This is the recommended mode for unit-testing parent stages that
/// dynamically create helper/child stages.
#[test]
fn virtual_child_stages() {
    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct ParentState {
        child_ref: Option<StageRef<u32>>,
        output: StageRef<u32>,
    }

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct ChildState {
        value: u32,
        output: StageRef<u32>,
    }

    let trace_buffer = TraceBuffer::new_shared(1, 1000000);
    let mut network = SimulationBuilder::default().with_trace_buffer(trace_buffer.clone());

    let parent = network.stage("parent", async |mut state: ParentState, msg: u32, eff| {
        if state.child_ref.is_none() {
            let child = eff
                .stage("child", async |mut state: ChildState, msg: u32, eff| {
                    // If this child transition ever executed, it would send to the output.
                    state.value += msg;
                    eff.send(&state.output, state.value).await;
                    state
                })
                .await;

            let child_ref = eff.wire_up(child, ChildState { value: 0, output: state.output.clone() }).await;
            state.child_ref = Some(child_ref);
        }

        if let Some(ref child) = state.child_ref {
            eff.send(child, msg).await;
        }

        state
    });

    let (output, mut rx) = network.output("output", 10);
    let parent = network.wire_up(parent, ParentState { child_ref: None, output: output.clone() });

    let mut running = network.run(test_runtime().handle());
    running.use_virtual_child_stages(true);

    running.enqueue_msg(&parent, [42u32]);
    running.run(Run::skip_wakeups()).assert_idle();

    // Because the child was virtual, its logic never ran → nothing reached the output.
    assert!(rx.drain().collect::<Vec<_>>().is_empty(), "virtual child must not produce any output");

    // Inspect the trace to verify the parent's creation effects were recorded.
    let trace = trace_buffer.lock().hydrate_without_timestamps();

    let has_add_stage = trace.iter().any(|e| matches!(e, TraceEntry::Suspend(Effect::AddStage { .. })));
    assert!(has_add_stage, "expected AddStage effect to be recorded");

    let has_wire_stage = trace.iter().any(|e| matches!(e, TraceEntry::Suspend(Effect::WireStage { .. })));
    assert!(has_wire_stage, "expected WireStage effect to be recorded");

    // The child's initial state should still have been pushed to the trace (via push_state).
    let has_child_state =
        trace.iter().any(|e| matches!(e, TraceEntry::State { stage, .. } if stage.as_str().starts_with("child")));
    assert!(has_child_state, "child's initial state should be recorded even in virtual mode");

    // Crucially, the child itself should never have become runnable (no Input for it).
    let child_inputs = trace
        .iter()
        .filter(|e| if let TraceEntry::Input { stage, .. } = e { stage.as_str().starts_with("child") } else { false })
        .count();
    assert_eq!(child_inputs, 0, "virtual child must never receive an input");

    // The parent *did* attempt to send a message to the virtual child ref (this send is visible in the trace).
    let has_send_to_child = trace.iter().any(|e| {
        if let TraceEntry::Suspend(Effect::Send { to, .. }) = e { to.as_str().starts_with("child") } else { false }
    });
    assert!(has_send_to_child, "parent should have sent to the virtual child ref");

    // The parent continued executing after the virtual wire_up (we see a later state snapshot for it).
    let has_later_parent_state =
        trace.iter().rev().any(|e| matches!(e, TraceEntry::State { stage, .. } if stage == parent.name()));
    assert!(has_later_parent_state, "parent should have continued and recorded state after the virtual wire_up");
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
enum Sum {
    N(u32),
}

#[test]
fn contramap_sends_injected_message_to_original_name() {
    let mut network = SimulationBuilder::default();
    let sink = network.stage("sink", async |mut state: Vec<Sum>, msg: Sum, _eff| {
        state.push(msg);
        state
    });
    let sink = network.wire_up(sink, Vec::new());
    let as_u32 = sink.contramap(Sum::N);
    assert_eq!(as_u32.name(), sink.name());

    for i in 0..50 {
        let _ = as_u32.contramap(move |_: ()| i);
    }

    network.preload(&as_u32, [7u32]).unwrap();
    let sender = network.input(&as_u32);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut running = network.run(rt.handle());
    running.run(Run::skip_wakeups()).assert_idle();
    assert_eq!(running.get_state(&sink), Some(&vec![Sum::N(7)]));

    drop(running);
    assert_eq!(rt.block_on(sender.send(1)), Err(amaru_pure_stage::SendError::new(sink.name().clone())));
}

fn parked_listener(
    label: &'static str,
) -> (amaru_pure_stage::simulation::SimulationRunning, amaru_pure_stage::stage_ref::StageStateRef<u32, u32>) {
    let mut network = SimulationBuilder::default();
    let stage = network.stage(label, async |mut state: u32, msg: u32, eff| {
        state += msg;
        let () = eff.external(Park).await;
        state
    });
    let stage = network.wire_up(stage, 0u32);
    let mut running = network.run(test_runtime().handle());
    running.enqueue_msg(&stage, [1]);
    (running, stage)
}

/// World-runner contract: UntilResolved stays Busy until `complete_external`.
#[test]
fn until_resolved_completed_later() {
    let (mut running, stage) = parked_listener("listener");
    running.run(Run::default()).assert_busy([stage.name()]).assert_external_effects(1);
    assert_eq!(running.get_state(&stage), None);
    running.complete_external(stage.name(), ());
    running.run(Run::default()).assert_idle();
    assert_eq!(*running.get_state(&stage).unwrap(), 1);
}

/// A stage parked on UntilResolved is Busy, not Idle.
#[test]
fn until_resolved_is_busy_not_idle() {
    let (mut running, stage) = parked_listener("listener");
    let blocked = running.run(Run::default());
    blocked.assert_busy([stage.name()]);
    assert_ne!(blocked, amaru_pure_stage::simulation::Blocked::Idle);
}

/// Two independent graphs may reuse stage names; completing one does not complete the other.
#[test]
fn two_graphs_are_independent() {
    let (mut a, sa) = parked_listener("listener");
    let (mut b, sb) = parked_listener("listener");
    assert_eq!(sa.name(), sb.name());
    a.run(Run::default()).assert_busy([sa.name()]);
    b.run(Run::default()).assert_busy([sb.name()]);
    a.complete_external(sa.name(), ());
    a.run(Run::default()).assert_idle();
    b.run(Run::default()).assert_busy([sb.name()]);
    b.complete_external(sb.name(), ());
    b.run(Run::default()).assert_idle();
}

/// Completing UntilResolved effects in offer order is the world's job; the crate does not reorder.
#[test]
fn complete_external_does_not_reorder() {
    let mut network = SimulationBuilder::default();
    let first = network.stage("first", async |mut state: u32, msg: u32, eff| {
        state += msg;
        let () = eff.external(Park).await;
        state
    });
    let second = network.stage("second", async |mut state: u32, msg: u32, eff| {
        state += msg;
        let () = eff.external(Park).await;
        state
    });
    let first = network.wire_up(first, 0u32);
    let second = network.wire_up(second, 0u32);
    let mut running = network.run(test_runtime().handle());
    running.enqueue_msg(&first, [1]);
    running.enqueue_msg(&second, [2]);
    running.run(Run::default()).assert_busy([first.name(), second.name()]);

    running.complete_external(second.name(), ());
    running.run(Run::default()).assert_busy([first.name()]);
    assert_eq!(*running.get_state(&second).unwrap(), 2);
    assert_eq!(running.get_state(&first), None);

    running.complete_external(first.name(), ());
    running.run(Run::default()).assert_idle();
    assert_eq!(*running.get_state(&first).unwrap(), 1);
}

/// A top-level terminate names that stage; a sibling is still present.
#[test]
fn terminated_names_one_stage() {
    let mut network = SimulationBuilder::default();
    let keeper = network.stage("keeper", async |state: u32, msg: u32, _eff| state + msg);
    let stopper = network.stage("stopper", async |state: (), _msg: u32, eff| {
        eff.terminate::<()>().await;
        state
    });
    let keeper = network.wire_up(keeper, 0u32);
    let stopper = network.wire_up(stopper, ());
    let mut running = network.run(test_runtime().handle());
    running.enqueue_msg(&stopper, [1]);
    running.run(Run::skip_wakeups()).assert_terminated(stopper.name());
    assert_eq!(*running.get_state(&keeper).unwrap(), 0);
}

/// A supervised helper can die without terminating the parent.
#[test]
fn supervised_helper_does_not_kill_parent() {
    let mut network = SimulationBuilder::default();
    let parent = network.stage("parent", async |mut child: Option<StageRef<u32>>, msg: u32, eff| {
        if child.is_none() {
            let helper = eff.stage("helper", async |(): (), _msg: u32, eff| eff.terminate::<()>().await).await;
            let helper = eff.supervise(helper, 99u32);
            let helper = eff.wire_up(helper, ()).await;
            child = Some(helper);
        }
        if msg == 1
            && let Some(helper) = child.as_ref()
        {
            eff.send(helper, 0).await;
        }
        child
    });
    let parent = network.wire_up(parent, None);
    let mut running = network.run(test_runtime().handle());
    running.enqueue_msg(&parent, [1]);
    running.run(Run::skip_wakeups()).assert_idle();
    assert!(!running.is_terminated());
    assert!(running.get_state(&parent).unwrap().is_some());
}

/// Completing an external on a stage that has already terminated panics.
#[test]
#[should_panic(expected = "does not exist")]
fn complete_external_on_gone_stage_panics() {
    let mut network = SimulationBuilder::default();
    let parent = network.stage("parent", async |mut child: Option<StageRef<u32>>, msg: u32, eff| {
        if child.is_none() {
            let helper = eff.stage("helper", async |(): (), _msg: u32, eff| eff.terminate::<()>().await).await;
            let helper = eff.supervise(helper, 99u32);
            let helper = eff.wire_up(helper, ()).await;
            child = Some(helper);
        }
        if msg == 1
            && let Some(helper) = child.as_ref()
        {
            eff.send(helper, 0).await;
        }
        child
    });
    let parent = network.wire_up(parent, None);
    let mut running = network.run(test_runtime().handle());
    running.enqueue_msg(&parent, [1]);
    running.run(Run::skip_wakeups()).assert_idle();
    let helper = running.get_state(&parent).unwrap().as_ref().unwrap().name().clone();
    running.complete_external(&helper, ());
}

/// Default `run` stops at a future wakeup; it does not fire the timer.
#[test]
fn default_run_does_not_skip_sleep() {
    let mut network = SimulationBuilder::default();
    let stage = network.stage("stage", async |state: (), _msg: u32, eff| {
        eff.set_timeout(Duration::from_secs(60), 1u32).await;
        state
    });
    let stage = network.wire_up(stage, ());
    let mut running = network.run(test_runtime().handle());
    running.enqueue_msg(&stage, [0u32]);
    let wakeup = running.run(Run::default()).assert_sleeping();
    assert_eq!(wakeup.sim_elapsed(), Duration::from_secs(60));
    assert_eq!(running.now().sim_elapsed(), Duration::ZERO);
}
