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

#![expect(clippy::unwrap_used, clippy::wildcard_enum_match_arm)]

//! Simulated duration of [`ExternalEffect`]: sampled `δ` is scheduled when the effect is
//! issued; [`DurationDist::UntilResolved`] waits for the Future instead.

use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use amaru_pure_stage::{
    DeserializerGuards, DurationDist, ExternalEffect, ExternalEffectAPI, Resources, StageGraph,
    assert_trace_does_not_contain, assert_trace_match, register_data_deserializer, register_effect_deserializer,
    simulation::SimulationBuilder, tm_clock, tm_clock_between, tm_external_effect, tm_input, tm_state,
    trace_buffer::TraceBuffer,
};

#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct ZeroWork;

impl ExternalEffect for ZeroWork {
    fn simulated_duration_dist(&self) -> DurationDist {
        <Self as ExternalEffectAPI>::SIMULATED_DURATION
    }

    fn run(
        self: Box<Self>,
        _resources: Resources,
    ) -> amaru_pure_stage::BoxFuture<'static, Box<dyn amaru_pure_stage::SendData>> {
        Self::wrap_sync(())
    }
}

impl ExternalEffectAPI for ZeroWork {
    type Response = ();
}

#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct ConstWork;

impl ExternalEffect for ConstWork {
    fn simulated_duration_dist(&self) -> DurationDist {
        <Self as ExternalEffectAPI>::SIMULATED_DURATION
    }

    fn run(
        self: Box<Self>,
        _resources: Resources,
    ) -> amaru_pure_stage::BoxFuture<'static, Box<dyn amaru_pure_stage::SendData>> {
        Self::wrap_sync(())
    }
}

impl ExternalEffectAPI for ConstWork {
    type Response = ();
    const SIMULATED_DURATION: DurationDist = DurationDist::Constant(Duration::from_secs(10));
}

#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct UniformWork;

impl ExternalEffect for UniformWork {
    fn simulated_duration_dist(&self) -> DurationDist {
        <Self as ExternalEffectAPI>::SIMULATED_DURATION
    }

    fn run(
        self: Box<Self>,
        _resources: Resources,
    ) -> amaru_pure_stage::BoxFuture<'static, Box<dyn amaru_pure_stage::SendData>> {
        Self::wrap_sync(())
    }
}

impl ExternalEffectAPI for UniformWork {
    type Response = ();
    const SIMULATED_DURATION: DurationDist =
        DurationDist::Uniform { min: Duration::from_secs(5), max: Duration::from_secs(15) };
}

#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct ResolvedWork;

impl ExternalEffect for ResolvedWork {
    fn simulated_duration_dist(&self) -> DurationDist {
        <Self as ExternalEffectAPI>::SIMULATED_DURATION
    }

    fn run(
        self: Box<Self>,
        _resources: Resources,
    ) -> amaru_pure_stage::BoxFuture<'static, Box<dyn amaru_pure_stage::SendData>> {
        Self::wrap_sync(())
    }
}

impl ExternalEffectAPI for ResolvedWork {
    type Response = ();
    const SIMULATED_DURATION: DurationDist = DurationDist::UntilResolved;
}

fn run_once<E: ExternalEffectAPI<Response = ()> + Default>(
    seed: u64,
) -> (amaru_pure_stage::simulation::SimulationRunning, DeserializerGuards) {
    let guards: DeserializerGuards = vec![
        register_data_deserializer::<()>().boxed(),
        register_data_deserializer::<u32>().boxed(),
        register_effect_deserializer::<ZeroWork>().boxed(),
        register_effect_deserializer::<ConstWork>().boxed(),
        register_effect_deserializer::<UniformWork>().boxed(),
        register_effect_deserializer::<ResolvedWork>().boxed(),
    ];

    let trace_buffer = TraceBuffer::new_shared(100, 1_000_000);
    let mut network = SimulationBuilder::default().with_trace_buffer(trace_buffer).with_seed(seed);

    let stage = network.stage("work", async |(), _msg: u32, eff| {
        eff.external(E::default()).await;
    });
    let stage = network.wire_up(stage, ());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut running = network.run(rt.handle());

    running.enqueue_msg(&stage, [1]);
    running.run_until_blocked_incl_effects().assert_idle();
    (running, guards)
}

#[test]
fn dyn_external_effect_projects_type_constant() {
    let zero: Box<dyn ExternalEffect> = Box::new(ZeroWork);
    let constant: Box<dyn ExternalEffect> = Box::new(ConstWork);
    let uniform: Box<dyn ExternalEffect> = Box::new(UniformWork);
    let resolved: Box<dyn ExternalEffect> = Box::new(ResolvedWork);
    assert_eq!(zero.simulated_duration_dist(), DurationDist::Zero);
    assert_eq!(constant.simulated_duration_dist(), DurationDist::Constant(Duration::from_secs(10)));
    assert_eq!(
        uniform.simulated_duration_dist(),
        DurationDist::Uniform { min: Duration::from_secs(5), max: Duration::from_secs(15) }
    );
    assert_eq!(resolved.simulated_duration_dist(), DurationDist::UntilResolved);
}

#[test]
fn zero_does_not_advance_the_clock() {
    let (running, _guards) = run_once::<ZeroWork>(1);
    assert_trace_does_not_contain(
        &running,
        &[tm_clock(Duration::ZERO), tm_clock_between(Duration::ZERO, Duration::from_secs(1000))],
    );
    let (running, _guards) = run_once::<ZeroWork>(1);
    assert_trace_match(
        &running,
        &[
            tm_state("work-1", &()),
            tm_input("work-1", &1u32),
            tm_external_effect::<ZeroWork>("work-1"),
            tm_state("work-1", &()),
        ],
    );
}

#[test]
fn constant_advances_the_clock_by_exactly_delta() {
    let (running, _guards) = run_once::<ConstWork>(1);
    assert_trace_match(
        &running,
        &[
            tm_state("work-1", &()),
            tm_input("work-1", &1u32),
            tm_external_effect::<ConstWork>("work-1"),
            tm_clock(Duration::from_secs(10)),
            tm_state("work-1", &()),
        ],
    );
}

#[test]
fn uniform_advances_the_clock_inside_the_declared_range() {
    let (running, _guards) = run_once::<UniformWork>(42);
    assert_trace_match(
        &running,
        &[
            tm_state("work-1", &()),
            tm_input("work-1", &1u32),
            tm_external_effect::<UniformWork>("work-1"),
            tm_clock_between(Duration::from_secs(5), Duration::from_secs(15)),
            tm_state("work-1", &()),
        ],
    );
}

#[test]
fn uniform_clock_is_deterministic_for_a_seed() {
    let (first, _g1) = run_once::<UniformWork>(7);
    let (second, _g2) = run_once::<UniformWork>(7);
    let clocks = |running: &amaru_pure_stage::simulation::SimulationRunning| {
        running
            .trace_buffer()
            .lock()
            .iter_entries()
            .filter_map(|(_, e)| match e {
                amaru_pure_stage::trace_buffer::TraceEntry::Clock(i) => Some(format!("{i}")),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(clocks(&first), clocks(&second));
    assert!(!clocks(&first).is_empty());
}

#[test]
fn sampled_delta_is_scheduled_when_the_effect_is_issued() {
    let _guards: DeserializerGuards = vec![
        register_data_deserializer::<()>().boxed(),
        register_data_deserializer::<u32>().boxed(),
        register_effect_deserializer::<ConstWork>().boxed(),
    ];
    let mut network = SimulationBuilder::default().with_seed(1);
    let stage = network.stage("work", async |(), _msg: u32, eff| {
        eff.external(ConstWork).await;
    });
    let stage = network.wire_up(stage, ());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut running = network.run(rt.handle());

    running.enqueue_msg(&stage, [1]);
    running.resume_receive(&stage).unwrap();
    let _effect = running.effect();

    // Wakeup exists before any result is provided — δ was sampled at issue time.
    assert_eq!(running.next_wakeup().map(|t| t.sim_elapsed()), Some(Duration::from_secs(10)));
    assert_eq!(running.now().sim_elapsed(), Duration::ZERO);

    running.resume_external::<ConstWork>(stage.name(), ()).unwrap();
    assert_eq!(running.now().sim_elapsed(), Duration::ZERO);
    running.run_until_blocked().assert_idle();
    assert_eq!(running.now().sim_elapsed(), Duration::from_secs(10));
}

#[test]
fn until_resolved_does_not_schedule_a_wakeup() {
    let _guards: DeserializerGuards = vec![
        register_data_deserializer::<()>().boxed(),
        register_data_deserializer::<u32>().boxed(),
        register_effect_deserializer::<ResolvedWork>().boxed(),
    ];
    let mut network = SimulationBuilder::default().with_seed(1);
    let stage = network.stage("work", async |(), _msg: u32, eff| {
        eff.external(ResolvedWork).await;
    });
    let stage = network.wire_up(stage, ());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut running = network.run(rt.handle());

    running.enqueue_msg(&stage, [1]);
    running.resume_receive(&stage).unwrap();
    let _effect = running.effect();

    assert_eq!(running.next_wakeup(), None);
    running.resume_external::<ResolvedWork>(stage.name(), ()).unwrap();
    running.effect().assert_receive(&stage);
    assert_eq!(running.now().sim_elapsed(), Duration::ZERO);
}

static COMPUTED_AT_DEADLINE: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct DeadlineWork;

impl ExternalEffect for DeadlineWork {
    fn simulated_duration_dist(&self) -> DurationDist {
        DurationDist::Constant(Duration::from_secs(10))
    }

    fn run(
        self: Box<Self>,
        _resources: Resources,
    ) -> amaru_pure_stage::BoxFuture<'static, Box<dyn amaru_pure_stage::SendData>> {
        Box::pin(async {
            COMPUTED_AT_DEADLINE.store(true, Ordering::SeqCst);
            Box::new(()) as Box<dyn amaru_pure_stage::SendData>
        })
    }
}

impl ExternalEffectAPI for DeadlineWork {
    type Response = ();
    const SIMULATED_DURATION: DurationDist = DurationDist::Constant(Duration::from_secs(10));
}

#[test]
fn sampled_deadline_forces_run_before_other_sim_steps() {
    COMPUTED_AT_DEADLINE.store(false, Ordering::SeqCst);
    let _guards: DeserializerGuards = vec![
        register_data_deserializer::<()>().boxed(),
        register_data_deserializer::<u32>().boxed(),
        register_effect_deserializer::<DeadlineWork>().boxed(),
    ];
    let mut network = SimulationBuilder::default().with_seed(1);
    let stage = network.stage("work", async |(), _msg: u32, eff| {
        eff.external(DeadlineWork).await;
    });
    let stage = network.wire_up(stage, ());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut running = network.run(rt.handle());

    running.enqueue_msg(&stage, [1]);
    running.resume_receive(&stage).unwrap();
    let effect = running.effect();
    running.handle_effect(effect);

    assert!(!COMPUTED_AT_DEADLINE.load(Ordering::SeqCst), "run() must not execute before the sampled deadline");
    assert_eq!(running.next_wakeup().map(|t| t.sim_elapsed()), Some(Duration::from_secs(10)));

    running.run_until_blocked().assert_idle();
    assert!(COMPUTED_AT_DEADLINE.load(Ordering::SeqCst), "run() must be forced when the deadline fires");
    assert_eq!(running.now().sim_elapsed(), Duration::from_secs(10));
}

#[test]
fn until_resolved_leaves_no_clock_entry() {
    let (running, _guards) = run_once::<ResolvedWork>(1);
    assert_trace_does_not_contain(&running, &[tm_clock_between(Duration::ZERO, Duration::from_secs(1000))]);
    let (running, _guards) = run_once::<ResolvedWork>(1);
    assert_trace_match(
        &running,
        &[
            tm_state("work-1", &()),
            tm_input("work-1", &1u32),
            tm_external_effect::<ResolvedWork>("work-1"),
            tm_state("work-1", &()),
        ],
    );
}
