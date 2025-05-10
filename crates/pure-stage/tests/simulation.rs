use pure_stage::{
    simulation::{Blocked, SimulationBuilder},
    Name, StageGraph, StageRef,
};
use tracing_subscriber::EnvFilter;

#[test]
fn basic() {
    let mut network = SimulationBuilder::default();
    let stage = network.stage(
        "basic",
        async |(mut state, out), msg: u32| {
            state += msg;
            out.send(state).await?;
            Ok((state, out))
        },
        (1u32, StageRef::<u32, ()>::noop()),
    );
    let (output, mut rx) = network.output("output");
    let stage = network.wire_up(stage, |state| state.1 = output);
    let mut running = network.run();

    // first check that the stages start receiving
    running.effect().assert_receive("output");
    running.effect().assert_receive("basic");
    running.try_effect().unwrap_err().assert_idle();

    // then insert some input and check reaction
    running.enqueue_msg(&stage, [1]);
    running.resume_receive("basic");
    running.effect().assert_send("basic", "output", 2u32);
    running.resume_send("basic", "output", 2u32);
    running.effect().assert_receive("basic");

    running.resume_receive("output");
    running.effect().assert_receive("output");

    assert_eq!(rx.drain().collect::<Vec<_>>(), vec![2]);
}

#[test]
fn automatic() {
    let mut network = SimulationBuilder::default();
    let stage = network.stage(
        "basic",
        async |(mut state, out), msg: u32| {
            state += msg;
            out.send(state).await?;
            Ok((state, out))
        },
        (1u32, StageRef::<u32, ()>::noop()),
    );
    let (output, mut rx) = network.output("output");
    let stage = network.wire_up(stage, |state| state.1 = output);
    let mut running = network.run();

    running.enqueue_msg(&stage, [1, 2, 3]);
    running.run_until_blocked().assert_idle();
    assert_eq!(rx.drain().collect::<Vec<_>>(), vec![2, 4, 7]);
}

#[test]
fn interrupt() {
    let mut network = SimulationBuilder::default();
    let interrupter = network.interrupter();
    let stage = network.stage(
        "basic",
        move |mut state, msg: u32| {
            state += msg;
            let interrupter = interrupter.clone();
            async move {
                if state > 5 {
                    interrupter.interrupt().await?;
                }
                Ok(state)
            }
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

    running.resume_interrupt("basic");
    running.run_until_blocked().assert_idle();
    assert_eq!(*running.get_state(&stage).unwrap(), 7);
}

#[test]
fn backpressure() {
    tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let mut network = SimulationBuilder::default().with_mailbox_size(1);
    let interrupter = network.interrupter();

    let sender = network.stage(
        "sender",
        async |target, msg: u32| {
            target.send(msg).await?;
            Ok(target)
        },
        StageRef::<u32, ()>::noop(),
    );

    let pressure = network.stage(
        "pressure",
        move |mut state, msg: u32| {
            state += msg;
            let interrupter = interrupter.clone();
            async move {
                if msg == 1 {
                    // this will block the stage and lead to backpressure
                    interrupter.interrupt().await?;
                }
                Ok(state)
            }
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

    running.resume_interrupt("pressure");
    running.run_until_blocked().assert_idle();
    assert_eq!(*running.get_state(&pressure).unwrap(), 7);
}
