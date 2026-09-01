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

//! [`Effects::detach`]: ack the airlock with `()`, enqueue the mapped `run()` value later.

use std::{sync::OnceLock, time::Duration};

use amaru_pure_stage::{
    BoxFuture, DeserializerGuards, DurationDist, ExternalEffectAPI, Resources, SendData, StageGraph, StageRef,
    assert_trace_contains, register_data_deserializer, register_effect_deserializer,
    simulation::{SimulationBuilder, running::OverrideResult},
    tm_external_effect_match, tm_input, tm_state,
    tokio::TokioBuilder,
    trace_buffer::TraceBuffer,
    trace_match::Detached,
};
use futures_util::StreamExt;
use tokio::{runtime::Runtime, time::timeout};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct Double(u32);

impl ExternalEffectAPI for Double {
    type Response = u32;

    fn run(self: Box<Self>, _resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap(|this| async move { this.0 * 2 })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct ConstDouble(u32);

impl ExternalEffectAPI for ConstDouble {
    type Response = u32;
    const SIMULATED_DURATION: DurationDist = DurationDist::Constant(Duration::from_secs(10));

    fn run(self: Box<Self>, _resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync(self.0 * 2)
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct UntilDouble(u32);

impl ExternalEffectAPI for UntilDouble {
    type Response = u32;
    const SIMULATED_DURATION: DurationDist = DurationDist::UntilResolved;

    fn run(self: Box<Self>, _resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync(self.0 * 2)
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct SleepDouble(u32);

impl ExternalEffectAPI for SleepDouble {
    type Response = u32;

    fn run(self: Box<Self>, _resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap(|this| async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            this.0 * 2
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
enum Msg {
    Go(u32),
    Done(u32),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct State {
    output: StageRef<u32>,
    acked: u32,
}

fn guards() -> DeserializerGuards {
    vec![
        register_data_deserializer::<()>().boxed(),
        register_data_deserializer::<u32>().boxed(),
        register_data_deserializer::<Msg>().boxed(),
        register_data_deserializer::<State>().boxed(),
        register_effect_deserializer::<Double>().boxed(),
        register_effect_deserializer::<ConstDouble>().boxed(),
        register_effect_deserializer::<UntilDouble>().boxed(),
        register_effect_deserializer::<SleepDouble>().boxed(),
    ]
}

#[expect(clippy::expect_used)]
fn test_runtime() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| Runtime::new().expect("tokio runtime"))
}

#[test]
fn simulation_detach_enqueues_mapped_result() {
    let _guards = guards();
    let trace_buffer = TraceBuffer::new_shared(100, 1_000_000);
    let mut network = SimulationBuilder::default().with_trace_buffer(trace_buffer);
    let work = network.stage("work", async |state: State, msg: Msg, eff| {
        match msg {
            Msg::Go(n) => {
                eff.detach(Double(n), Msg::Done).await;
                eff.send(&state.output, 0).await;
            }
            Msg::Done(v) => {
                eff.send(&state.output, v).await;
            }
        }
        state
    });
    let (output, mut rx) = network.output("output", 10);
    let work = network.wire_up(work, State { output: output.clone(), acked: 0 });
    let mut running = network.run(test_runtime().handle());

    running.enqueue_msg(&work, [Msg::Go(21)]);
    running.run_until_blocked_incl_effects().assert_idle();
    assert_eq!(rx.drain().collect::<Vec<_>>(), vec![0, 42]);

    assert_trace_contains(
        &running,
        &[
            tm_state("work-1", &State { output: output.clone(), acked: 0 }),
            tm_input("work-1", &Msg::Go(21)),
            tm_external_effect_match::<Double>("work-1", |_| true, Detached::Yes),
            tm_input("work-1", &Msg::Done(42)),
        ],
    );
}

#[test]
fn simulation_detach_override_is_injected() {
    let _guards = guards();
    let mut network = SimulationBuilder::default();
    let work = network.stage("work", async |state: State, msg: Msg, eff| {
        match msg {
            Msg::Go(n) => eff.detach(Double(n), Msg::Done).await,
            Msg::Done(v) => eff.send(&state.output, v).await,
        }
        state
    });
    let (output, mut rx) = network.output("output", 10);
    let work = network.wire_up(work, State { output, acked: 0 });
    let mut running = network.run(test_runtime().handle());

    running.override_external_effect::<Double>(1, |_| OverrideResult::handled(99));
    running.enqueue_msg(&work, [Msg::Go(1)]);
    running.run_until_blocked_incl_effects().assert_idle();
    assert_eq!(rx.drain().collect::<Vec<_>>(), vec![99]);
}

#[test]
fn simulation_two_detaches_in_one_transition() {
    let _guards = guards();
    let mut network = SimulationBuilder::default();
    let work = network.stage("work", async |state: State, msg: Msg, eff| {
        match msg {
            Msg::Go(_) => {
                eff.detach(Double(1), Msg::Done).await;
                eff.detach(Double(2), Msg::Done).await;
            }
            Msg::Done(v) => eff.send(&state.output, v).await,
        }
        state
    });
    let (output, mut rx) = network.output("output", 10);
    let work = network.wire_up(work, State { output, acked: 0 });
    let mut running = network.run(test_runtime().handle());

    running.enqueue_msg(&work, [Msg::Go(0)]);
    running.run_until_blocked_incl_effects().assert_idle();
    assert_eq!(rx.drain().collect::<Vec<_>>(), vec![2, 4]);
}

#[test]
fn simulation_detach_constant_delays_mailbox_not_ack() {
    let _guards = guards();
    let mut network = SimulationBuilder::default().with_seed(1);
    let work = network.stage("work", async |state: State, msg: Msg, eff| {
        match msg {
            Msg::Go(n) => {
                eff.detach(ConstDouble(n), Msg::Done).await;
                eff.send(&state.output, 0).await;
            }
            Msg::Done(v) => eff.send(&state.output, v).await,
        }
        state
    });
    let (output, mut rx) = network.output("output", 10);
    let work = network.wire_up(work, State { output, acked: 0 });
    let mut running = network.run(test_runtime().handle());

    running.enqueue_msg(&work, [Msg::Go(3)]);
    let wakeup = running.run_until_sleeping_or_blocked().assert_sleeping();
    assert_eq!(wakeup.sim_elapsed(), Duration::from_secs(10));
    assert_eq!(rx.drain().collect::<Vec<_>>(), vec![0]);
    assert_eq!(running.now().sim_elapsed(), Duration::ZERO);

    running.run_until_blocked_incl_effects().assert_idle();
    assert_eq!(rx.drain().collect::<Vec<_>>(), vec![6]);
    assert_eq!(running.now().sim_elapsed(), Duration::from_secs(10));
}

#[test]
fn simulation_detach_until_resolved_is_busy_until_polled() {
    let _guards = guards();
    let mut network = SimulationBuilder::default();
    let work = network.stage("work", async |state: State, msg: Msg, eff| {
        match msg {
            Msg::Go(n) => eff.detach(UntilDouble(n), Msg::Done).await,
            Msg::Done(v) => eff.send(&state.output, v).await,
        }
        state
    });
    let (output, mut rx) = network.output("output", 10);
    let work = network.wire_up(work, State { output, acked: 0 });
    let mut running = network.run(test_runtime().handle());

    running.enqueue_msg(&work, [Msg::Go(7)]);
    let blocked = running.run_until_blocked();
    blocked.assert_busy(std::iter::empty::<&str>());
    blocked.assert_external_effects(1);
    assert!(rx.drain().collect::<Vec<_>>().is_empty());

    running.run_until_blocked_incl_effects().assert_idle();
    assert_eq!(rx.drain().collect::<Vec<_>>(), vec![14]);
}

#[test]
fn tokio_detach_completes_after_stage_continues() {
    let rt = Runtime::new().unwrap();
    let mut graph = TokioBuilder::default();
    let work = graph.stage("work", async |state: State, msg: Msg, eff| {
        match msg {
            Msg::Go(n) => {
                eff.detach(SleepDouble(n), Msg::Done).await;
                eff.send(&state.output, 0).await;
            }
            Msg::Done(v) => {
                eff.send(&state.output, v).await;
            }
        }
        state
    });
    let (out_ref, mut out_rx) = graph.output("output", 10);
    let work = graph.wire_up(work, State { output: out_ref, acked: 0 });
    let send = graph.input(&work);

    let handle = rt.handle().clone();
    rt.block_on(async move {
        let graph = graph.run(handle);
        timeout(Duration::from_secs(1), send.send(Msg::Go(21))).await.unwrap().unwrap();
        assert_eq!(timeout(Duration::from_secs(1), out_rx.next()).await.unwrap(), Some(0));
        assert_eq!(timeout(Duration::from_secs(1), out_rx.next()).await.unwrap(), Some(42));
        graph.abort();
    });
}
