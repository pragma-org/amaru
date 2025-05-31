use pure_stage::{
    simulation::{OverrideResult, SimulationBuilder},
    CallRef, Effect, ExternalEffect, Message, OutputEffect, StageGraph,
};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tracing_subscriber::EnvFilter;

#[test]
fn basic() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut network = SimulationBuilder::default();
    let basic = network.stage("basic", async |(mut state, out), msg: u32, eff| {
        state += msg;
        eff.send(&out, state).await;
        Ok((state, out))
    });
    let (output, mut rx) = network.output("output", 10);
    let basic = network.wire_up(basic, (1u32, output.without_state()));
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
    assert_eq!(&*result, &() as &dyn Message);
    running.resume_external(&output, result).unwrap();
    running.effect().assert_receive(&output);

    assert_eq!(rx.drain().collect::<Vec<_>>(), vec![2]);
}

#[test]
fn automatic() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut network = SimulationBuilder::default();
    let basic = network.stage("basic", async |(mut state, out), msg: u32, eff| {
        state += msg;
        eff.send(&out, state).await;
        Ok((state, out))
    });
    let (output, mut rx) = network.output("output", 10);
    let basic = network.wire_up(basic, (1u32, output.without_state()));
    let mut running = network.run(rt.handle().clone());

    running.enqueue_msg(&basic, [1, 2, 3]);
    running.run_until_blocked().assert_idle();
    assert_eq!(rx.drain().collect::<Vec<_>>(), vec![2, 4, 7]);
}

#[test]
fn breakpoint() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut network = SimulationBuilder::default();
    let basic = network.stage("basic", async |(mut state, out), msg: u32, eff| {
        state += msg;
        eff.send(&out, state).await;
        Ok((state, out))
    });
    let (output, mut rx) = network.output("output", 10);
    let basic = network.wire_up(basic, (1u32, output.without_state()));
    let mut running = network.run(rt.handle().clone());

    running.enqueue_msg(&basic, [1, 2, 3]);
    running.breakpoint("send4", move |eff| {
        matches!(
            eff,
            Effect::Send { from, to, msg, .. }
                if from == &basic.name &&
                    to == &output.name &&
                    *msg == Box::new(4u32) as Box<dyn Message>
        )
    });
    running.run_until_blocked().assert_breakpoint("send4");
    assert_eq!(rx.drain().collect::<Vec<_>>(), vec![2]);
}

#[test]
fn overrides() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut network = SimulationBuilder::default();
    let basic = network.stage("basic", async |(mut state, out), msg: u32, eff| {
        state += msg;
        eff.send(&out, state).await;
        Ok((state, out))
    });
    let (output, mut rx) = network.output("output", 10);
    let basic = network.wire_up(basic, (1u32, output.without_state()));
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

#[test]
fn clock() {
    let mut network = SimulationBuilder::default();
    let basic = network.stage("basic", async |_state, msg: u32, eff| {
        let now = eff.clock().await;
        let later = eff.wait(Duration::from_secs(1)).await;
        Ok(Some((msg, now, later)))
    });
    let basic = network.wire_up(basic, None);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut running = network.run(rt.handle().clone());

    running.enqueue_msg(&basic, [42]);
    let now = running.now();
    running.run_until_blocked().assert_idle();
    let later = running.now();
    assert_eq!(
        running.get_state(&basic).unwrap(),
        &Some((42u32, now, later))
    );
    assert_eq!(later.checked_since(now).unwrap(), Duration::from_secs(1));
}

#[test]
fn clock_manual() {
    let mut network = SimulationBuilder::default();
    let stage = network.stage("basic", async |_state, msg: u32, eff| {
        let now = eff.clock().await;
        let later = eff.wait(Duration::from_secs(1)).await;
        Ok(Some((msg, now, later)))
    });
    let stage = network.wire_up(stage, None);
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
    let caller = network.stage("caller", async |(_state, target), msg: u32, eff| {
        let state = eff
            .call(&target, Duration::from_secs(2), move |cr| (msg + 1, cr))
            .await
            .ok_or_else(|| anyhow::anyhow!("call timed out"))?;
        Ok((state, target))
    });

    let callee = network.stage("callee", async |state, msg: (u32, CallRef<u32>), eff| {
        eff.wait(Duration::from_secs(1)).await;
        eff.respond(msg.1, msg.0 * 2).await;
        Ok(state)
    });
    let caller = network.wire_up(caller, (1u32, callee.sender()));
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
