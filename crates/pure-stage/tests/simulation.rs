use pure_stage::{
    simulation::{Blocked, SimulationBuilder},
    CallRef, Name, StageGraph, StageRef,
};
use std::time::Duration;
use tracing_subscriber::EnvFilter;

#[test]
fn basic() {
    let mut network = SimulationBuilder::default();
    let stage = network.stage(
        "basic",
        async |(mut state, out), msg: u32, eff| {
            state += msg;
            eff.send(&out, state).await;
            Ok((state, out))
        },
        (1u32, StageRef::noop::<u32>()),
    );
    let (output, mut rx) = network.output("output");
    let stage = network.wire_up(stage, |state| state.1 = output.without_state());
    let mut running = network.run();

    // first check that the stages start out suspended on Receive
    running.try_effect().unwrap_err().assert_idle();

    // then insert some input and check reaction
    running.enqueue_msg(&stage, [1]);
    running.resume_receive(&stage).unwrap();
    running.effect().assert_send(&stage, &output, 2u32);
    running.resume_send(&stage, &output, 2u32).unwrap();
    running.effect().assert_receive(&stage);

    running.resume_receive(&output).unwrap();
    running.effect().assert_receive(&output);

    assert_eq!(rx.drain().collect::<Vec<_>>(), vec![2]);
}

#[test]
fn automatic() {
    let mut network = SimulationBuilder::default();
    let stage = network.stage(
        "basic",
        async |(mut state, out), msg: u32, eff| {
            state += msg;
            eff.send(&out, state).await;
            Ok((state, out))
        },
        (1u32, StageRef::noop::<u32>()),
    );
    let (output, mut rx) = network.output("output");
    let stage = network.wire_up(stage, |state| state.1 = output.without_state());
    let mut running = network.run();

    running.enqueue_msg(&stage, [1, 2, 3]);
    running.run_until_blocked().assert_idle();
    assert_eq!(rx.drain().collect::<Vec<_>>(), vec![2, 4, 7]);
}

#[test]
fn interrupt() {
    let mut network = SimulationBuilder::default();
    let stage = network.stage(
        "basic",
        async |mut state, msg: u32, eff| {
            state += msg;
            if state > 5 {
                eff.interrupt().await;
            }
            Ok(state)
        },
        1u32,
    );
    let stage = network.wire_up(stage, |_| {});
    let mut running = network.run();

    running.enqueue_msg(&stage, [1, 2, 3]);
    running.run_until_blocked().assert_interrupted("basic");
    assert_eq!(
        running.run_until_blocked(),
        Blocked::Busy(vec![Name::from("basic")])
    );

    running.resume_interrupt(&stage).unwrap();
    running.run_until_blocked().assert_idle();
    assert_eq!(*running.get_state(&stage).unwrap(), 7);
}

#[test]
fn backpressure() {
    tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init()
        .ok();

    let mut network = SimulationBuilder::default().with_mailbox_size(1);

    let sender = network.stage(
        "sender",
        async |target, msg: u32, eff| {
            eff.send(&target, msg).await;
            Ok(target)
        },
        StageRef::noop::<u32>(),
    );

    let pressure = network.stage(
        "pressure",
        async |mut state, msg: u32, eff| {
            state += msg;
            if msg == 1 {
                // this will block the stage and lead to backpressure
                eff.interrupt().await;
            }
            Ok(state)
        },
        1u32,
    );

    let sender = network.wire_up(sender, |state| *state = pressure.sender());
    let pressure = network.wire_up(pressure, |_| {});

    let mut running = network.run();

    running.enqueue_msg(&sender, [1, 2, 3]);
    running.run_until_blocked().assert_interrupted("pressure");
    // not resuming here so that "sender" will continue to run
    running.run_until_blocked().assert_busy(["pressure"]);

    running.resume_interrupt(&pressure).unwrap();
    running.run_until_blocked().assert_idle();
    assert_eq!(*running.get_state(&pressure).unwrap(), 7);
}

#[test]
fn clock() {
    let mut network = SimulationBuilder::default();
    let stage = network.stage(
        "basic",
        async |_state, msg: u32, eff| {
            let now = eff.clock().await;
            let later = eff.wait(Duration::from_secs(1)).await;
            Ok(Some((msg, now, later)))
        },
        None,
    );
    let stage = network.wire_up(stage, |_| {});
    let mut running = network.run();

    running.enqueue_msg(&stage, [42]);
    let now = running.now();
    running.run_until_blocked().assert_idle();
    let later = running.now();
    assert_eq!(
        running.get_state(&stage).unwrap(),
        &Some((42u32, now, later))
    );
    assert_eq!(later.checked_since(now).unwrap(), Duration::from_secs(1));
}

#[test]
fn clock_manual() {
    let mut network = SimulationBuilder::default();
    let stage = network.stage(
        "basic",
        async |_state, msg: u32, eff| {
            let now = eff.clock().await;
            let later = eff.wait(Duration::from_secs(1)).await;
            Ok(Some((msg, now, later)))
        },
        None,
    );
    let stage = network.wire_up(stage, |_| {});
    let mut running = network.run();

    running.enqueue_msg(&stage, [42]);
    let now = running.now();
    running.run_until_sleeping_or_blocked().assert_sleeping();
    assert_eq!(running.get_state(&stage), None);

    assert!(running.skip_to_next_wakeup());
    running.run_until_sleeping_or_blocked().assert_idle();
    let later = running.now();

    assert_eq!(
        running.get_state(&stage).unwrap(),
        &Some((42u32, now, later))
    );
    assert_eq!(later.checked_since(now).unwrap(), Duration::from_secs(1));
}

#[test]
fn call() {
    tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init()
        .ok();

    let mut network = SimulationBuilder::default();
    let caller = network.stage(
        "caller",
        async |(_state, target), msg: u32, eff| {
            let state = eff
                .call(&target, Duration::from_secs(2), move |cr| (msg + 1, cr))
                .await
                .ok_or_else(|| anyhow::anyhow!("call timed out"))?;
            Ok((state, target))
        },
        (1u32, StageRef::noop::<(u32, CallRef<u32>)>()),
    );

    let callee = network.stage(
        "callee",
        async |state, msg: (u32, CallRef<u32>), eff| {
            eff.wait(Duration::from_secs(1)).await;
            eff.respond(msg.1, msg.0 * 2).await;
            Ok(state)
        },
        (),
    );
    let caller = network.wire_up(caller, |state| state.1 = callee.sender());
    let callee = network.wire_up(callee, |_| {});

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
        |(msg, cr)| (msg + 1, cr),
        Duration::from_secs(2),
    );

    let cr2 = cr.dummy();
    sim.resume_send(&caller, &callee, (msg, cr)).unwrap();
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
