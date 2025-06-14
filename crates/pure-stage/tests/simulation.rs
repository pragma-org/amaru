#![allow(clippy::bool_assert_comparison)]

use pure_stage::{
    simulation::{OverrideResult, SimulationBuilder},
    trace_buffer::TraceBuffer,
    CallRef, Effect, ExternalEffect, Instant, OutputEffect, Receiver, SendData, StageGraph,
    StageRef, Void,
};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tracing_subscriber::EnvFilter;

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct State(u32, StageRef<u32, Void>);

#[test]
fn basic() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut network = SimulationBuilder::default();
    let basic = network.stage("basic", async |mut state: State, msg: u32, eff| {
        state.0 += msg;
        eff.send(&state.1, state.0).await;
        Ok(state)
    });
    let (output, mut rx) = network.output("output", 10);
    let basic = network.wire_up(basic, State(1u32, output.without_state()));
    let mut running = network.run(rt.handle().clone());

    // first check that the stages start out suspended on Receive
    running.try_effect().unwrap_err().assert_idle();

    // then insert some input and check reaction
    running.enqueue_msg(&basic, [1]);
    running.resume_receive(&basic).unwrap();
    running.effect().assert_send(&basic, &output, 2u32);
    running.resume_send(&basic, &output, 2u32).unwrap();
    running.effect().assert_receive(&basic);

    running.resume_receive(&output).unwrap();
    let ext = running
        .effect()
        .extract_external(&output, &OutputEffect::fake(output.name(), 2u32).0);
    let result = rt.block_on(ext.run());
    // this check is also done when resuming, just want to show how to do it here
    assert_eq!(&*result, &() as &dyn SendData);
    running.resume_external(&output, result).unwrap();
    running.effect().assert_receive(&output);

    assert_eq!(rx.drain().collect::<Vec<_>>(), vec![2]);
}

#[test]
fn automatic() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let trace_buffer = TraceBuffer::new_shared(100, 1_000_000);
    let mut network = SimulationBuilder::default().with_trace_buffer(trace_buffer.clone());

    fn basic(
        network: &mut impl StageGraph,
    ) -> (StageRef<u32, Void>, Receiver<u32>, StageRef<u32, Void>) {
        let basic = network.stage("basic", async |mut state: State, msg: u32, eff| {
            state.0 += msg;
            eff.wait(Duration::from_secs(10)).await;
            eff.send(&state.1, state.0).await;
            Ok(state)
        });
        let (output, rx) = network.output("output", 10);
        let basic = network.wire_up(basic, State(1u32, output.without_state()));
        (basic.without_state(), rx, output)
    }

    let (in_ref, mut rx, output) = basic(&mut network);
    let mut running = network.run(rt.handle().clone());

    running.enqueue_msg(&in_ref, [1, 2, 3]);
    running.run_until_blocked().assert_idle();
    assert_eq!(rx.drain().collect::<Vec<_>>(), vec![2, 4, 7]);

    let trace = trace_buffer.lock().hydrate();

    const EXPECTED: &[&str] = &[
        "State { stage: Name(\"basic-0\"), state: SendDataValue { typetag: \"simulation::State\", value: Array([Integer(1), Map([(Text(\"name\"), Text(\"output-1\"))])]) } }",
        "State { stage: Name(\"output-1\"), state: SendDataValue { typetag: \"pure_stage::types::MpscSender<u32>\", value: Map([]) } }",
        "Input { stage: Name(\"basic-0\"), input: SendDataValue { typetag: \"u32\", value: Integer(1) } }",
        "Resume { stage: Name(\"basic-0\"), response: Unit }",
        "Suspend(Wait { at_stage: Name(\"basic-0\"), duration: 10s })",
        "Clock(Instant(10s))",
        "Resume { stage: Name(\"basic-0\"), response: WaitResponse(Instant(10s)) }",
        "Suspend(Send { from: Name(\"basic-0\"), to: Name(\"output-1\"), msg: SendDataValue { typetag: \"u32\", value: Integer(2) }, call: None })",
        "Input { stage: Name(\"output-1\"), input: SendDataValue { typetag: \"u32\", value: Integer(2) } }",
        "Resume { stage: Name(\"output-1\"), response: Unit }",
        "Suspend(External { at_stage: Name(\"output-1\"), effect: UnknownExternalEffect { value: SendDataValue { typetag: \"pure_stage::output::OutputEffect<u32>\", value: Map([(Text(\"name\"), Text(\"output-1\")), (Text(\"msg\"), Integer(2)), (Text(\"sender\"), Map([]))]) } } })",
        "Resume { stage: Name(\"basic-0\"), response: Unit }",
        "State { stage: Name(\"basic-0\"), state: SendDataValue { typetag: \"simulation::State\", value: Array([Integer(2), Map([(Text(\"name\"), Text(\"output-1\"))])]) } }",
        "Suspend(Receive { at_stage: Name(\"basic-0\") })",
        "Input { stage: Name(\"basic-0\"), input: SendDataValue { typetag: \"u32\", value: Integer(2) } }",
        "Resume { stage: Name(\"output-1\"), response: ExternalResponse(SendDataValue { typetag: \"()\", value: Array([]) }) }",
        "State { stage: Name(\"output-1\"), state: SendDataValue { typetag: \"pure_stage::types::MpscSender<u32>\", value: Map([]) } }",
        "Suspend(Receive { at_stage: Name(\"output-1\") })",
        "Resume { stage: Name(\"basic-0\"), response: Unit }",
        "Suspend(Wait { at_stage: Name(\"basic-0\"), duration: 10s })",
        "Clock(Instant(20s))",
        "Resume { stage: Name(\"basic-0\"), response: WaitResponse(Instant(20s)) }",
        "Suspend(Send { from: Name(\"basic-0\"), to: Name(\"output-1\"), msg: SendDataValue { typetag: \"u32\", value: Integer(4) }, call: None })",
        "Input { stage: Name(\"output-1\"), input: SendDataValue { typetag: \"u32\", value: Integer(4) } }",
        "Resume { stage: Name(\"output-1\"), response: Unit }",
        "Suspend(External { at_stage: Name(\"output-1\"), effect: UnknownExternalEffect { value: SendDataValue { typetag: \"pure_stage::output::OutputEffect<u32>\", value: Map([(Text(\"name\"), Text(\"output-1\")), (Text(\"msg\"), Integer(4)), (Text(\"sender\"), Map([]))]) } } })",
        "Resume { stage: Name(\"basic-0\"), response: Unit }",
        "State { stage: Name(\"basic-0\"), state: SendDataValue { typetag: \"simulation::State\", value: Array([Integer(4), Map([(Text(\"name\"), Text(\"output-1\"))])]) } }",
        "Suspend(Receive { at_stage: Name(\"basic-0\") })",
        "Input { stage: Name(\"basic-0\"), input: SendDataValue { typetag: \"u32\", value: Integer(3) } }",
        "Resume { stage: Name(\"output-1\"), response: ExternalResponse(SendDataValue { typetag: \"()\", value: Array([]) }) }",
        "State { stage: Name(\"output-1\"), state: SendDataValue { typetag: \"pure_stage::types::MpscSender<u32>\", value: Map([]) } }",
        "Suspend(Receive { at_stage: Name(\"output-1\") })",
        "Resume { stage: Name(\"basic-0\"), response: Unit }",
        "Suspend(Wait { at_stage: Name(\"basic-0\"), duration: 10s })",
        "Clock(Instant(30s))",
        "Resume { stage: Name(\"basic-0\"), response: WaitResponse(Instant(30s)) }",
        "Suspend(Send { from: Name(\"basic-0\"), to: Name(\"output-1\"), msg: SendDataValue { typetag: \"u32\", value: Integer(7) }, call: None })",
        "Input { stage: Name(\"output-1\"), input: SendDataValue { typetag: \"u32\", value: Integer(7) } }",
        "Resume { stage: Name(\"output-1\"), response: Unit }",
        "Suspend(External { at_stage: Name(\"output-1\"), effect: UnknownExternalEffect { value: SendDataValue { typetag: \"pure_stage::output::OutputEffect<u32>\", value: Map([(Text(\"name\"), Text(\"output-1\")), (Text(\"msg\"), Integer(7)), (Text(\"sender\"), Map([]))]) } } })",
        "Resume { stage: Name(\"basic-0\"), response: Unit }",
        "State { stage: Name(\"basic-0\"), state: SendDataValue { typetag: \"simulation::State\", value: Array([Integer(7), Map([(Text(\"name\"), Text(\"output-1\"))])]) } }",
        "Suspend(Receive { at_stage: Name(\"basic-0\") })",
        "Resume { stage: Name(\"output-1\"), response: ExternalResponse(SendDataValue { typetag: \"()\", value: Array([]) }) }",
        "State { stage: Name(\"output-1\"), state: SendDataValue { typetag: \"pure_stage::types::MpscSender<u32>\", value: Map([]) } }",
        "Suspend(Receive { at_stage: Name(\"output-1\") })",
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
        replay.latest_state(&in_ref.name()),
        Some(&State(7, output.without_state()) as &dyn SendData)
    );
    assert_eq!(replay.is_running(&in_ref.name()), false);
    assert_eq!(replay.is_idle(&in_ref.name()), true);
    assert_eq!(replay.is_failed(&output.name()), false);
    assert_eq!(replay.is_idle(&output.name()), true);
    assert_eq!(replay.get_failure(&in_ref.name()), None);
    assert_eq!(replay.get_failure(&output.name()), None);
    assert_eq!(replay.clock(), Instant::at_offset(Duration::from_secs(30)));
}

#[test]
fn breakpoint() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut network = SimulationBuilder::default();
    let basic = network.stage("basic", async |mut state: State, msg: u32, eff| {
        state.0 += msg;
        eff.send(&state.1, state.0).await;
        Ok(state)
    });
    let (output, mut rx) = network.output("output", 10);
    let basic = network.wire_up(basic, State(1u32, output.without_state()));
    let mut running = network.run(rt.handle().clone());

    running.enqueue_msg(&basic, [1, 2, 3]);
    running.breakpoint("send4", move |eff| {
        matches!(
            eff,
            Effect::Send { from, to, msg, .. }
                if from == &basic.name &&
                    to == &output.name &&
                    *msg == Box::new(4u32) as Box<dyn SendData>
        )
    });
    running.run_until_blocked().assert_breakpoint("send4");
    assert_eq!(rx.drain().collect::<Vec<_>>(), vec![2]);
}

#[test]
fn overrides() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut network = SimulationBuilder::default();
    let basic = network.stage("basic", async |mut state: State, msg: u32, eff| {
        state.0 += msg;
        eff.send(&state.1, state.0).await;
        Ok(state)
    });
    let (output, mut rx) = network.output("output", 10);
    let basic = network.wire_up(basic, State(1u32, output.without_state()));
    let mut running = network.run(rt.handle().clone());

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
    running.run_until_blocked().assert_idle();
    assert_eq!(rx.drain().collect::<Vec<_>>(), vec![2, 7]);
    assert_eq!(count.load(Ordering::Relaxed), 1);
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
        Ok(target)
    });

    let pressure = network.stage("pressure", async |mut state, msg: u32, eff| {
        state += msg;
        // we need to place an effect that we can install a breakpoint on
        // other than Receive, because that is automatically resumed upon sending
        eff.clock().await;
        Ok(state)
    });

    let sender = network.wire_up(sender, pressure.sender());
    let pressure = network.wire_up(pressure, 1u32);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut running = network.run(rt.handle().clone());

    running.enqueue_msg(&sender, [1, 2, 3]);
    running.breakpoint("pressure", {
        let pressure = pressure.clone();
        move |eff| matches!(eff, Effect::Clock { at_stage: a } if a == &pressure.name)
    });

    let broken = running.run_until_blocked().assert_breakpoint("pressure");
    assert_eq!(
        broken,
        Effect::Clock {
            at_stage: pressure.name(),
        }
    );

    running.run_until_blocked().assert_busy([&pressure.name]);

    running.handle_effect(broken);
    running.clear_breakpoint("pressure");
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
        Ok(State2::Full(msg, now, later))
    });
    let basic = network.wire_up(basic, State2::Empty);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut running = network.run(rt.handle().clone());

    running.enqueue_msg(&basic, [42]);
    let now = running.now();
    running.run_until_blocked().assert_idle();
    let later = running.now();
    assert_eq!(
        running.get_state(&basic).unwrap(),
        &State2::Full(42u32, now, later)
    );
    assert_eq!(later.checked_since(now).unwrap(), Duration::from_secs(1));
}

#[test]
fn clock_manual() {
    let mut network = SimulationBuilder::default();
    let stage = network.stage("basic", async |_state, msg: u32, eff| {
        let now = eff.clock().await;
        let later = eff.wait(Duration::from_secs(1)).await;
        Ok(State2::Full(msg, now, later))
    });
    let stage = network.wire_up(stage, State2::Empty);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut running = network.run(rt.handle().clone());

    running.enqueue_msg(&stage, [42]);
    let now = running.now();
    running.run_until_sleeping_or_blocked().assert_sleeping();
    assert_eq!(running.get_state(&stage), None);

    assert!(running.skip_to_next_wakeup());
    running.run_until_sleeping_or_blocked().assert_idle();
    let later = running.now();

    assert_eq!(
        running.get_state(&stage).unwrap(),
        &State2::Full(42u32, now, later)
    );
    assert_eq!(later.checked_since(now).unwrap(), Duration::from_secs(1));
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct State3(u32, StageRef<Msg3, Void>);

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct Msg3(u32, CallRef<u32>);

#[test]
fn call() {
    tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init()
        .ok();

    let mut network = SimulationBuilder::default();
    let caller = network.stage("caller", async |mut state: State3, msg: u32, eff| {
        state.0 = eff
            .call(&state.1, Duration::from_secs(2), move |cr| {
                Msg3(msg + 1, cr)
            })
            .await
            .ok_or_else(|| anyhow::anyhow!("call timed out"))?;
        Ok(state)
    });

    let callee = network.stage("callee", async |state, msg: Msg3, eff| {
        eff.wait(Duration::from_secs(1)).await;
        eff.respond(msg.1, msg.0 * 2).await;
        Ok(state)
    });
    let caller = network.wire_up(caller, State3(1u32, callee.sender()));
    let callee = network.wire_up(callee, ());

    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut sim = network.run(rt.handle().clone());

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

    let cr2 = cr.dummy();
    sim.resume_send(&caller, &callee, Msg3(msg, cr)).unwrap();
    // still not runnable, now waiting for response
    sim.try_effect().unwrap_err().assert_busy([caller.name()]);

    sim.resume_receive(&callee).unwrap();
    sim.effect().assert_wait(&callee, Duration::from_secs(1));
    sim.resume_wait(&callee, sim.now()).unwrap();
    sim.effect().assert_respond(&callee, &cr2, 8);
    sim.resume_respond(&callee, &cr2, 7).unwrap();
    sim.effect().assert_receive(&callee);
    // the processing above has already dealt with sending the response, which has resumed the caller
    sim.effect().assert_receive(&caller);
    assert_eq!(sim.get_state(&caller).unwrap().0, 7);
}
