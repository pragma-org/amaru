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

use pure_stage::{
    Effect, ExternalEffect, Instant, Name, OutputEffect, Receiver, Resources, SendData, StageGraph,
    StageGraphRunning, StageRef, StageResponse, TryInStage, UnknownExternalEffect,
    serde::SendDataValue,
    simulation::{RandStdRng, SimulationBuilder, running::OverrideResult},
    trace_buffer::{TraceBuffer, TraceEntry},
};
use rand::{SeedableRng, rngs::StdRng};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, Waker},
    time::Duration,
};
use tracing_subscriber::EnvFilter;

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct State(u32, StageRef<u32>);

#[test]
fn basic() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut network = SimulationBuilder::default();
    let basic = network.stage("basic", async |mut state: State, msg: u32, eff| {
        state.0 += msg;
        eff.send(&state.1, state.0).await;
        state
    });
    let (output, mut rx) = network.output("output", 10);
    let basic = network.wire_up(basic, State(1u32, output.clone()));
    let mut running = network.run();

    // first check that the stages start out suspended on Receive
    running.try_effect().unwrap_err().assert_idle();

    // then insert some input and check reaction
    running.enqueue_msg(&basic, [1]);
    running.resume_receive(&basic).unwrap();
    running.effect().assert_send(&basic, &output, 2u32);
    running.resume_send(&basic, &output, Some(2u32)).unwrap();
    running.effect().assert_receive(&basic);

    running.resume_receive(&output).unwrap();
    let ext = running
        .effect()
        .extract_external::<OutputEffect<u32>>(&output);
    assert_eq!(&*ext, &OutputEffect::fake(output.name().clone(), 2u32).0);
    let result = rt.block_on(ext.run(Resources::default()));
    // this check is also done when resuming, just want to show how to do it here
    assert_eq!(&*result, &() as &dyn SendData);
    running.resume_external_box(&output, result).unwrap();
    running.effect().assert_receive(&output);

    assert_eq!(rx.drain().collect::<Vec<_>>(), vec![2]);
}

#[test]
fn automatic() {
    let trace_buffer = TraceBuffer::new_shared(100, 1_000_000);
    let std_rng = StdRng::from_seed([0; 32]);
    let mut network = SimulationBuilder::default()
        .with_trace_buffer(trace_buffer.clone())
        .with_eval_strategy(RandStdRng(std_rng));

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
    let mut running = network.run();
    let rt = tokio::runtime::Runtime::new().unwrap();

    running.enqueue_msg(&in_ref, [1, 2, 3]);
    running
        .run_until_blocked_incl_effects(rt.handle())
        .assert_idle();
    assert_eq!(rx.drain().collect::<Vec<_>>(), vec![2, 4, 7]);

    let trace = trace_buffer.lock().hydrate_without_timestamps();

    const EXPECTED: &[&str] = &[
        "State { stage: Name(\"basic-1\"), state: SendDataValue { typetag: \"simulation::State\", value: Array([Integer(1), Map([(Text(\"name\"), Text(\"output-2\"))])]) } }",
        "State { stage: Name(\"output-2\"), state: SendDataValue { typetag: \"pure_stage::types::MpscSender<u32>\", value: Map([]) } }",
        "Input { stage: Name(\"basic-1\"), input: SendDataValue { typetag: \"u32\", value: Integer(1) } }",
        "Resume { stage: Name(\"basic-1\"), response: Unit }",
        "Suspend(Wait { at_stage: Name(\"basic-1\"), duration: 10s })",
        "Clock(Instant(10s))",
        "Resume { stage: Name(\"basic-1\"), response: WaitResponse(Instant(10s)) }",
        "Suspend(Send { from: Name(\"basic-1\"), to: Name(\"output-2\"), msg: SendDataValue { typetag: \"u32\", value: Integer(2) } })",
        "Input { stage: Name(\"output-2\"), input: SendDataValue { typetag: \"u32\", value: Integer(2) } }",
        "Resume { stage: Name(\"output-2\"), response: Unit }",
        "Suspend(External { at_stage: Name(\"output-2\"), effect: UnknownExternalEffect { value: SendDataValue { typetag: \"pure_stage::output::OutputEffect<u32>\", value: Map([(Text(\"name\"), Text(\"output-2\")), (Text(\"msg\"), Integer(2)), (Text(\"sender\"), Map([]))]) } } })",
        "Resume { stage: Name(\"basic-1\"), response: Unit }",
        "State { stage: Name(\"basic-1\"), state: SendDataValue { typetag: \"simulation::State\", value: Array([Integer(2), Map([(Text(\"name\"), Text(\"output-2\"))])]) } }",
        "Input { stage: Name(\"basic-1\"), input: SendDataValue { typetag: \"u32\", value: Integer(2) } }",
        "Resume { stage: Name(\"basic-1\"), response: Unit }",
        "Suspend(Wait { at_stage: Name(\"basic-1\"), duration: 10s })",
        "Resume { stage: Name(\"output-2\"), response: ExternalResponse(SendDataValue { typetag: \"()\", value: Array([]) }) }",
        "State { stage: Name(\"output-2\"), state: SendDataValue { typetag: \"pure_stage::types::MpscSender<u32>\", value: Map([]) } }",
        "Clock(Instant(20s))",
        "Resume { stage: Name(\"basic-1\"), response: WaitResponse(Instant(20s)) }",
        "Suspend(Send { from: Name(\"basic-1\"), to: Name(\"output-2\"), msg: SendDataValue { typetag: \"u32\", value: Integer(4) } })",
        "Input { stage: Name(\"output-2\"), input: SendDataValue { typetag: \"u32\", value: Integer(4) } }",
        "Resume { stage: Name(\"output-2\"), response: Unit }",
        "Suspend(External { at_stage: Name(\"output-2\"), effect: UnknownExternalEffect { value: SendDataValue { typetag: \"pure_stage::output::OutputEffect<u32>\", value: Map([(Text(\"name\"), Text(\"output-2\")), (Text(\"msg\"), Integer(4)), (Text(\"sender\"), Map([]))]) } } })",
        "Resume { stage: Name(\"basic-1\"), response: Unit }",
        "State { stage: Name(\"basic-1\"), state: SendDataValue { typetag: \"simulation::State\", value: Array([Integer(4), Map([(Text(\"name\"), Text(\"output-2\"))])]) } }",
        "Input { stage: Name(\"basic-1\"), input: SendDataValue { typetag: \"u32\", value: Integer(3) } }",
        "Resume { stage: Name(\"basic-1\"), response: Unit }",
        "Suspend(Wait { at_stage: Name(\"basic-1\"), duration: 10s })",
        "Resume { stage: Name(\"output-2\"), response: ExternalResponse(SendDataValue { typetag: \"()\", value: Array([]) }) }",
        "State { stage: Name(\"output-2\"), state: SendDataValue { typetag: \"pure_stage::types::MpscSender<u32>\", value: Map([]) } }",
        "Clock(Instant(30s))",
        "Resume { stage: Name(\"basic-1\"), response: WaitResponse(Instant(30s)) }",
        "Suspend(Send { from: Name(\"basic-1\"), to: Name(\"output-2\"), msg: SendDataValue { typetag: \"u32\", value: Integer(7) } })",
        "Input { stage: Name(\"output-2\"), input: SendDataValue { typetag: \"u32\", value: Integer(7) } }",
        "Resume { stage: Name(\"output-2\"), response: Unit }",
        "Suspend(External { at_stage: Name(\"output-2\"), effect: UnknownExternalEffect { value: SendDataValue { typetag: \"pure_stage::output::OutputEffect<u32>\", value: Map([(Text(\"name\"), Text(\"output-2\")), (Text(\"msg\"), Integer(7)), (Text(\"sender\"), Map([]))]) } } })",
        "Resume { stage: Name(\"basic-1\"), response: Unit }",
        "State { stage: Name(\"basic-1\"), state: SendDataValue { typetag: \"simulation::State\", value: Array([Integer(7), Map([(Text(\"name\"), Text(\"output-2\"))])]) } }",
        "Resume { stage: Name(\"output-2\"), response: ExternalResponse(SendDataValue { typetag: \"()\", value: Array([]) }) }",
        "State { stage: Name(\"output-2\"), state: SendDataValue { typetag: \"pure_stage::types::MpscSender<u32>\", value: Map([]) } }",
    ];

    pretty_assertions::assert_eq!(
        trace.iter().map(|t| format!("{t:?}")).collect::<Vec<_>>(),
        EXPECTED
    );

    let mut network = SimulationBuilder::default();
    basic(&mut network);
    let mut replay = network.replay();
    replay.run_trace(trace).unwrap();

    assert_eq!(
        replay.latest_state(in_ref.name()),
        Some(&State(7, output.clone()) as &dyn SendData)
    );
    assert_eq!(replay.is_running(in_ref.name()), false);
    assert_eq!(replay.is_idle(in_ref.name()), true);
    assert_eq!(replay.is_terminating(output.name()), false);
    assert_eq!(replay.is_idle(output.name()), true);
    assert_eq!(replay.clock(), Instant::at_offset(Duration::from_secs(30)));
}

#[test]
fn breakpoint() {
    let std_rng = StdRng::from_seed([0; 32]);
    let mut network = SimulationBuilder::default().with_eval_strategy(RandStdRng(std_rng));
    let basic = network.stage("basic", async |mut state: State, msg: u32, eff| {
        state.0 += msg;
        eff.send(&state.1, state.0).await;
        state
    });
    let (output, mut rx) = network.output("output", 10);
    let basic = network.wire_up(basic, State(1u32, output.clone()));
    let mut running = network.run();
    let rt = tokio::runtime::Runtime::new().unwrap();

    running.enqueue_msg(&basic, [1, 2, 3]);
    let output2 = output.clone();
    running.breakpoint("send4", move |eff| {
        matches!(
            eff,
            Effect::Send { from, to, msg, .. }
                if from == basic.name() &&
                    to == output.name() &&
                    *msg == Box::new(4u32) as Box<dyn SendData>
        )
    });
    running.run_until_blocked().assert_breakpoint("send4");
    assert_eq!(
        &rt.block_on(running.await_external_effect()).unwrap(),
        output2.name()
    );
    assert_eq!(rt.block_on(running.await_external_effect()), None);
    running.effect().assert_receive(&output2);
    running
        .try_effect()
        .unwrap_err()
        .assert_deadlock(["basic-1"]);
    assert_eq!(rx.drain().collect::<Vec<_>>(), vec![2]);
}

#[test]
fn overrides() {
    let _guard = pure_stage::register_data_deserializer::<State>();
    let _guard = pure_stage::register_data_deserializer::<u32>();
    let _guard = pure_stage::register_effect_deserializer::<OutputEffect<u32>>();

    tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init()
        .ok();

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
    let mut running = network.run();
    let rt = tokio::runtime::Runtime::new().unwrap();

    let count = Arc::new(AtomicUsize::new(0));
    let count2 = count.clone();
    running.enqueue_msg(&basic, [1, 2, 3]);
    running.override_external_effect(1, move |eff: Box<OutputEffect<u32>>| {
        if eff.msg > 2 {
            count2.fetch_add(1, Ordering::Relaxed);
            OverrideResult::Handled(Box::new(()))
        } else {
            OverrideResult::NoMatch(eff)
        }
    });
    running
        .run_until_blocked_incl_effects(rt.handle())
        .assert_idle();
    assert_eq!(rx.drain().collect::<Vec<_>>(), vec![2, 7]);
    assert_eq!(count.load(Ordering::Relaxed), 1);

    guard.defuse();
}

#[test]
fn backpressure() {
    tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init()
        .ok();

    let mut network = SimulationBuilder::default().with_mailbox_size(1);

    let sender = network.stage("sender", async |target, msg: u32, eff| {
        eff.send(&target, msg).await;
        target
    });

    let pressure = network.stage("pressure", async |mut state, msg: u32, eff| {
        state += msg;
        // we need to place an effect that we can install a breakpoint on
        // other than Receive, because that is automatically resumed upon sending
        eff.clock().await;
        state
    });

    let sender = network.wire_up(sender, pressure.sender());
    let pressure = network.wire_up(pressure, 1u32);

    let mut running = network.run();

    running.enqueue_msg(&sender, [1]);
    running.breakpoint("pressure", {
        let pressure = pressure.clone();
        move |eff| matches!(eff, Effect::Clock { at_stage: a } if a == pressure.name())
    });

    let sender_name = sender.name().clone();
    running.breakpoint(
        "send",
        move |eff| matches!(eff, Effect::Receive { at_stage } if *at_stage == sender_name),
    );

    let broken = running.run_until_blocked().assert_breakpoint("pressure");
    assert_eq!(
        broken,
        Effect::Clock {
            at_stage: pressure.name().clone(),
        }
    );

    running.run_until_blocked().assert_breakpoint("send");
    running.enqueue_msg(&sender, [2]);
    running.resume_receive(&sender).unwrap();

    running.run_until_blocked().assert_breakpoint("send");
    running.enqueue_msg(&sender, [3]);
    running.resume_receive(&sender).unwrap();

    // backpressure is here: "send" breakpoint is not yet hit because waiting to send to `pressure`
    running.run_until_blocked().assert_busy([pressure.name()]);
    running.handle_effect(broken);

    running.clear_breakpoint("pressure");

    running.run_until_blocked().assert_breakpoint("send");
    running.run_until_blocked().assert_idle();

    assert_eq!(*running.get_state(&pressure).unwrap(), 7);
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
    let mut running = network.run();

    running.enqueue_msg(&basic, [42]);
    let now = running.now();
    running.run_until_blocked().assert_idle();
    let later = running.now();
    assert_eq!(
        running.get_state(&basic).unwrap(),
        &State2::Full(42u32, now, later)
    );
    assert_eq!(later.checked_since(now).unwrap(), Duration::from_secs(1));

    running.enqueue_msg(&basic, [43]);
    let wakeup = running
        .run_until_blocked_or_time(later + Duration::from_millis(100))
        .assert_sleeping();
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
    let mut running = network.run();

    running.enqueue_msg(&stage, [42]);
    let now = running.now();
    running.run_until_sleeping_or_blocked().assert_sleeping();
    assert_eq!(running.get_state(&stage), None);

    let intermediate = running.now() + Duration::from_millis(100);
    let target = intermediate + Duration::from_millis(900);

    assert!(!running.skip_to_next_wakeup(Some(intermediate)));
    assert_eq!(running.now(), intermediate);

    assert!(running.skip_to_next_wakeup(None));
    assert_eq!(running.now(), target);

    running.run_until_sleeping_or_blocked().assert_idle();
    let later = running.now();

    assert_eq!(
        running.get_state(&stage).unwrap(),
        &State2::Full(42u32, now, later)
    );
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
    tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init()
        .ok();

    let _guard = pure_stage::register_data_deserializer::<Msg3>();
    let trace_buffer = TraceBuffer::new_shared(1, 1000000);
    let guard = TraceBuffer::drop_guard(&trace_buffer);

    let mut network = SimulationBuilder::default().with_trace_buffer(trace_buffer);
    let caller = network.stage("caller", async |mut state: State3, msg: u32, eff| {
        state.0 = eff
            .call(&state.1, Duration::from_secs(2), move |cr| {
                Msg3(msg + 1, cr)
            })
            .await
            .or_terminate(&eff, async |_| ())
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

    let mut sim = network.run();

    sim.enqueue_msg(&caller, [1]);
    sim.run_until_blocked().assert_idle();
    assert_eq!(sim.get_state(&caller).unwrap().0, 4);

    // also try manual mode
    sim.enqueue_msg(&caller, [2]);
    sim.resume_receive(&caller).unwrap();
    let (msg, cr) = sim.effect().assert_call(
        &caller,
        &callee,
        |msg| (msg.0 + 1, msg.1),
        Duration::from_secs(2),
    );

    sim.try_effect().unwrap_err().assert_busy([caller.name()]);
    // this will resume receive on callee
    sim.resume_call_send(&caller, &callee, Msg3(msg, cr.clone()))
        .unwrap();
    sim.effect().assert_wait(&callee, Duration::from_secs(1));
    sim.resume_wait(&callee, sim.now()).unwrap();
    sim.effect().assert_send(&callee, &cr, 8);
    sim.resume_send(&callee, &cr, Some(7)).unwrap();
    sim.effect().assert_receive(&callee);
    // the processing above has already dealt with sending the response, which has resumed the caller
    sim.effect().assert_receive(&caller);
    assert_eq!(sim.get_state(&caller).unwrap().0, 7);

    guard.defuse();
}

#[test]
fn call_timeout_terminates_graph() {
    let _guard = pure_stage::register_data_deserializer::<Msg3>();
    let trace_buffer = TraceBuffer::new_shared(1, 1000000);
    let guard = TraceBuffer::drop_guard(&trace_buffer);
    let mut network = SimulationBuilder::default().with_trace_buffer(trace_buffer);

    // caller times out quickly; callee sleeps longer -> triggers terminate
    let caller = network.stage("caller", async |state: State3, msg: u32, eff| {
        eff.call(&state.1, Duration::from_millis(10), move |cr| {
            Msg3(msg + 1, cr)
        })
        .await
        // Returning terminate here should trigger graph termination
        // (SimulationRunning.termination should complete)
        .or_terminate(&eff, async |_| {})
        .await;
        state
    });

    let callee = network.stage("callee", async |state, _msg: Msg3, eff| {
        eff.wait(Duration::from_secs(1)).await; // Ensure we exceed caller timeout
        state
    });

    let caller = network.wire_up(caller, State3(0u32, callee.sender()));
    network.wire_up(callee, ());

    let mut sim = network.run();

    sim.enqueue_msg(&caller, [1]);
    // Run until blocked, then assert termination flips true
    let mut term = sim.termination();
    assert_eq!(
        term.as_mut().poll(&mut Context::from_waker(Waker::noop())),
        Poll::Pending
    );

    sim.run_until_blocked().assert_terminated(caller.name()); // drive effects

    assert!(sim.is_terminated(), "simulation should report terminated");
    assert_eq!(
        term.as_mut().poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(())
    );

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

    let trace_buffer = TraceBuffer::new_shared(1, 1000000);
    let mut network = SimulationBuilder::default().with_trace_buffer(trace_buffer.clone());

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
            let child_ref = eff
                .wire_up(
                    child,
                    ChildState {
                        value: 0u32,
                        output: state.output.clone(),
                    },
                )
                .await;
            state.child_ref = Some(child_ref);
        }

        // Send a message to the child stage
        if let Some(ref child) = state.child_ref {
            eff.send(child, msg).await;
        }

        state
    });

    let (output, mut rx) = network.output("output", 10);
    let parent = network.wire_up(
        parent,
        ParentState {
            child_ref: None,
            output: output.clone(),
        },
    );
    let mut running = network.run();

    let rt = tokio::runtime::Runtime::new().unwrap();

    // Initially, parent is waiting for Receive
    running.try_effect().unwrap_err().assert_idle();

    // Send a message to the parent to trigger child stage creation
    running.enqueue_msg(&parent, [42]);
    running.resume_receive(&parent).unwrap();

    // Assert that AddStage effect is emitted
    let add_stage_effect = running.effect();
    add_stage_effect.assert_add_stage(&parent, "child");
    let child_ref = StageRef::named_for_tests("child-12");

    running
        .resume_add_stage(&parent, child_ref.name().clone())
        .unwrap();

    // Assert that WireStage effect is emitted
    let wire_stage_effect = running.effect();
    wire_stage_effect.assert_wire_stage(
        &parent,
        "child-12",
        ChildState {
            value: 0u32,
            output: output.clone(),
        },
    );

    // Resume the WireStage effect with the child's initial state
    running.handle_effect(wire_stage_effect);

    // Now the parent should send a message to the child
    let send_effect = running.effect();
    send_effect.assert_send(&parent, &child_ref, 42u32);
    running.handle_effect(send_effect);

    // The child should send a message to the output (as per its transition function)
    let child_send_effect = running.effect();
    child_send_effect.assert_send(&child_ref, &output, 42u32);
    running.handle_effect(child_send_effect);

    running.effect().assert_receive(&parent);

    let external_effect = running.effect();
    external_effect.assert_external(&output, &OutputEffect::fake(output.name().clone(), 42u32).0);
    running.handle_effect(external_effect);

    running.effect().assert_receive(&child_ref);
    running
        .try_effect()
        .unwrap_err()
        .assert_busy(["output-2"])
        .assert_external_effects(1);
    assert_eq!(
        &rt.block_on(running.await_external_effect()).unwrap(),
        output.name()
    );
    running.effect().assert_receive(&output);

    // Verify output received the message from the child stage
    assert_eq!(rx.drain().collect::<Vec<_>>(), vec![42]);

    // verify trace buffer
    pretty_assertions::assert_eq!(
        trace_buffer.lock().hydrate_without_timestamps(),
        vec![
            TraceEntry::state(
                "output-2",
                SendDataValue::from_json(
                    "pure_stage::types::MpscSender<u32>",
                    BTreeMap::<u8, u8>::new()
                )
            ),
            TraceEntry::state(
                parent.name(),
                SendDataValue::boxed(&ParentState {
                    child_ref: None,
                    output: output.clone()
                })
            ),
            TraceEntry::input(parent.name(), SendDataValue::boxed(&42u32)),
            TraceEntry::resume(parent.name(), StageResponse::Unit),
            TraceEntry::suspend(Effect::AddStage {
                at_stage: parent.name().clone(),
                name: Name::from("child")
            }),
            TraceEntry::resume(
                parent.name(),
                StageResponse::AddStageResponse(child_ref.name().clone())
            ),
            TraceEntry::suspend(Effect::wire_stage(
                parent.name().clone(),
                child_ref.name().clone(),
                SendDataValue::boxed(&ChildState {
                    value: 0u32,
                    output: output.clone()
                }),
                None
            )),
            TraceEntry::state(
                child_ref.name(),
                SendDataValue::boxed(&ChildState {
                    value: 0,
                    output: output.clone()
                })
            ),
            TraceEntry::resume(parent.name(), StageResponse::Unit),
            TraceEntry::suspend(Effect::Send {
                from: parent.name().clone(),
                to: child_ref.name().clone(),
                msg: SendDataValue::boxed(&42u32),
            }),
            TraceEntry::input(child_ref.name(), SendDataValue::boxed(&42u32)),
            TraceEntry::resume(child_ref.name(), StageResponse::Unit),
            TraceEntry::suspend(Effect::Send {
                from: child_ref.name().clone(),
                to: output.name().clone(),
                msg: SendDataValue::boxed(&42u32),
            }),
            TraceEntry::input(output.name(), SendDataValue::boxed(&42u32)),
            TraceEntry::resume(parent.name(), StageResponse::Unit),
            TraceEntry::state(
                parent.name(),
                SendDataValue::boxed(&ParentState {
                    child_ref: Some(child_ref.clone()),
                    output: output.clone()
                })
            ),
            TraceEntry::resume(output.name(), StageResponse::Unit),
            TraceEntry::suspend(Effect::External {
                at_stage: output.name().clone(),
                effect: UnknownExternalEffect::boxed(
                    &OutputEffect::fake(output.name().clone(), 42u32).0
                )
            }),
            TraceEntry::resume(child_ref.name(), StageResponse::Unit),
            TraceEntry::state(
                child_ref.name(),
                SendDataValue::boxed(&ChildState {
                    value: 42u32,
                    output: output.clone()
                })
            ),
            TraceEntry::resume(
                output.name(),
                StageResponse::ExternalResponse(SendDataValue::boxed(&()))
            ),
            TraceEntry::state(
                output.name(),
                SendDataValue::from_json(
                    "pure_stage::types::MpscSender<u32>",
                    BTreeMap::<u8, u8>::new()
                )
            ),
        ]
    );
}
