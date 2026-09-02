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
    DeserializerGuards, DurationDist, ExternalEffect, ExternalEffectAPI, Resources, StageGraph, assert_trace_contains,
    assert_trace_does_not_contain, assert_trace_match, assert_trace_match_filter, register_data_deserializer,
    register_effect_deserializer,
    simulation::{Run, SimulationBuilder},
    tm_clock, tm_clock_between, tm_effect, tm_external_effect, tm_external_effect_any, tm_input, tm_resume,
    tm_resume_external, tm_resume_unit, tm_state,
    trace_buffer::TraceBuffer,
};

#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct ZeroWork;

impl ExternalEffectAPI for ZeroWork {
    type Response = ();

    fn run(
        self: Box<Self>,
        _resources: Resources,
    ) -> amaru_pure_stage::BoxFuture<'static, Box<dyn amaru_pure_stage::SendData>> {
        self.wrap_sync(())
    }
}

#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct ConstWork;

impl ExternalEffectAPI for ConstWork {
    type Response = ();
    const SIMULATED_DURATION: DurationDist = DurationDist::Constant(Duration::from_secs(10));

    fn run(
        self: Box<Self>,
        _resources: Resources,
    ) -> amaru_pure_stage::BoxFuture<'static, Box<dyn amaru_pure_stage::SendData>> {
        self.wrap_sync(())
    }
}

#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct UniformWork;

impl ExternalEffectAPI for UniformWork {
    type Response = ();
    const SIMULATED_DURATION: DurationDist =
        DurationDist::Uniform { min: Duration::from_secs(5), max: Duration::from_secs(15) };

    fn run(
        self: Box<Self>,
        _resources: Resources,
    ) -> amaru_pure_stage::BoxFuture<'static, Box<dyn amaru_pure_stage::SendData>> {
        self.wrap_sync(())
    }
}

#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct ResolvedWork;

impl ExternalEffectAPI for ResolvedWork {
    type Response = ();
    const SIMULATED_DURATION: DurationDist = DurationDist::UntilResolved;

    fn run(
        self: Box<Self>,
        _resources: Resources,
    ) -> amaru_pure_stage::BoxFuture<'static, Box<dyn amaru_pure_stage::SendData>> {
        self.wrap_sync(())
    }
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
    running.run(Run::skip_and_resolve()).assert_idle();
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
fn tm_external_effect_any_matches_regardless_of_stage_name() {
    let (running, _guards) = run_once::<ZeroWork>(1);
    assert_trace_contains(&running, &[tm_external_effect_any::<ZeroWork>()]);
}

#[test]
fn assert_trace_match_filter_drops_matched_actuals() {
    let (running, _guards) = run_once::<ZeroWork>(1);
    assert_trace_match_filter(
        &running,
        &[
            tm_state("work-1", &()),
            tm_input("work-1", &1u32),
            tm_resume_unit("work-1"),
            tm_effect("work-1", ZeroWork),
            tm_resume_external("work-1", ()),
            tm_state("work-1", &()),
        ],
        &[],
    );

    let (running, _guards) = run_once::<ZeroWork>(1);
    assert_trace_match_filter(
        &running,
        &[tm_input("work-1", &1u32), tm_effect("work-1", ZeroWork)],
        &[tm_resume(), tm_state("work-1", &())],
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
    running.run(Run::default()).assert_sleeping();

    // Wakeup exists before the result is forced — δ was sampled at issue time.
    assert_eq!(running.next_wakeup().map(|t| t.sim_elapsed()), Some(Duration::from_secs(10)));
    assert_eq!(running.now().sim_elapsed(), Duration::ZERO);

    running.run(Run::skip_wakeups()).assert_idle();
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
    running.run(Run::default()).assert_busy(["work"]);

    assert_eq!(running.next_wakeup(), None);
    running.complete_external(stage.name(), ());
    running.run(Run::default()).assert_idle();
    assert_eq!(running.now().sim_elapsed(), Duration::ZERO);
}

static COMPUTED_AT_DEADLINE: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct DeadlineWork;

impl ExternalEffectAPI for DeadlineWork {
    type Response = ();
    const SIMULATED_DURATION: DurationDist = DurationDist::Constant(Duration::from_secs(10));

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

#[cfg(debug_assertions)]
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct MismatchedDuration;

#[cfg(debug_assertions)]
impl ExternalEffectAPI for MismatchedDuration {
    type Response = ();
    const SIMULATED_DURATION: DurationDist = DurationDist::UntilResolved;

    fn simulated_duration_dist(&self) -> DurationDist {
        DurationDist::ZERO
    }

    fn run(
        self: Box<Self>,
        _resources: Resources,
    ) -> amaru_pure_stage::BoxFuture<'static, Box<dyn amaru_pure_stage::SendData>> {
        self.wrap_sync(())
    }
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "must return ExternalEffectAPI::SIMULATED_DURATION")]
fn wrap_rejects_duration_dist_mismatch() {
    drop(ExternalEffectAPI::run(Box::new(MismatchedDuration), Resources::default()));
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
    running.run(Run::default()).assert_sleeping();

    assert!(!COMPUTED_AT_DEADLINE.load(Ordering::SeqCst), "run() must not execute before the sampled deadline");
    assert_eq!(running.next_wakeup().map(|t| t.sim_elapsed()), Some(Duration::from_secs(10)));

    running.run(Run::skip_wakeups()).assert_idle();
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
