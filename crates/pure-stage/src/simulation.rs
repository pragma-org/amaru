#![expect(
    clippy::wildcard_enum_match_arm,
    clippy::unwrap_used,
    clippy::panic,
    clippy::expect_used
)]
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

//! This module contains the [`SimulationBuilder`] and [`SimulationRunning`] types, which are
//! used to build and run a simulation.
//!
//! The simulation is a fully controllable and deterministic [`StageGraph`](crate::StageGraph) for testing purposes.
//! Execution is controlled entirely via the [`SimulationRunning`] handle returned from
//! [`StageGraph::run`](crate::StageGraph::run).
//!

use crate::{
    BoxFuture, Effects, Instant, Name, Resources, SendData, Sender, Stage, StageBuildRef, StageRef,
    effect::{StageEffect, StageResponse},
    time::Clock,
    trace_buffer::TraceBuffer,
};
use either::Either;
use parking_lot::Mutex;
use std::{
    collections::{BTreeMap, VecDeque},
    future::poll_fn,
    marker::PhantomData,
    sync::{Arc, atomic::AtomicU64},
    task::Poll,
};
use tokio::runtime::Handle;

pub use blocked::Blocked;
pub use replay::Replay;
pub use running::{OverrideResult, SimulationRunning};

use crate::stage_ref::StageStateRef;
use inputs::Inputs;
use state::{InitStageData, InitStageState, StageData, StageState, Transition};

mod blocked;
mod inputs;
mod replay;
mod resume;
mod running;
mod state;

pub(crate) type EffectBox =
    Arc<Mutex<Option<Either<StageEffect<Box<dyn SendData>>, StageResponse>>>>;

pub(crate) fn airlock_effect<Out>(
    eb: &EffectBox,
    effect: StageEffect<Box<dyn SendData>>,
    mut response: impl FnMut(Option<StageResponse>) -> Option<Out> + Send + 'static,
) -> BoxFuture<'static, Out> {
    let eb = eb.clone();
    let mut effect = Some(effect);
    Box::pin(poll_fn(move |_| {
        let mut eb = eb.lock();
        if let Some(effect) = effect.take() {
            match eb.take() {
                Some(Either::Left(x)) => panic!("effect already set: {:?}", x),
                // it is either Some(Right(Unit)) after Receive or None otherwise
                Some(Either::Right(StageResponse::Unit)) | None => {}
                Some(Either::Right(resp)) => {
                    panic!("effect airlock contains leftover response: {:?}", resp)
                }
            }
            *eb = Some(Either::Left(effect));
            Poll::Pending
        } else {
            let Some(out) = eb.take() else {
                return Poll::Pending;
            };
            let out = match out {
                Either::Left(x) => panic!("expected response, got effect: {:?}", x),
                Either::Right(x) => response(Some(x)),
            };
            out.map(Poll::Ready).unwrap_or(Poll::Pending)
        }
    }))
}

/// A fully controllable and deterministic [`StageGraph`](crate::StageGraph) for testing purposes.
///
/// Execution is controlled entirely via the [`SimulationRunning`] handle returned from
/// [`StageGraph::run`](crate::StageGraph::run).
///
/// The general principle is that each stage is suspended whenever it needs new
/// input (even when there is a message available in the mailbox) or when it uses
/// any of the effects provided (like [`Effects::send`] or [`Effects::wait`]).
/// Resuming the given effect will not run the stage, but it will make it runnable
/// again when performing the next simulation step.
///
/// Example:
/// ```rust
/// use pure_stage::{Resources, Stage, StageGraph, StageRef, simulation::SimulationBuilder, Effects, OutputEffect, ExternalEffect};
/// use tokio::runtime::Runtime;
/// use async_trait::async_trait;
///
/// let mut network = SimulationBuilder::default();
///
/// // Create the graph
/// let stage = network.make_stage("basic");
/// let output = network.make_stage("output");
///
/// #[derive(Clone)]
/// struct MyStage { out: StageRef<u32> };
///
/// impl MyStage {
///   pub fn new(out: impl AsRef<StageRef<u32>>) -> Self { Self { out: out.as_ref().clone() } }
/// }
///
/// #[async_trait]
/// impl Stage<u32, u32> for MyStage {
///     fn initial_state(&self) -> u32 { 1u32 }
///
///     async fn run(&self, mut state: u32, msg: u32, eff: Effects<u32>) -> u32 {
///         state += msg;
///         eff.send(&self.out, state).await;
///         state
///     }
/// }
///
/// network.register(&stage, MyStage::new(&output));
/// let mut rx = network.output(&output, 10);
///
/// // Start the network. This needs a Tokio runtime for executing external effects.
/// let rt = Runtime::new().unwrap();
/// let mut running = network.run(rt.handle().clone());
///
/// // First check that the stages start out suspended on Receive
/// running.try_effect().unwrap_err().assert_idle();
///
/// // Then insert some input and check reaction
/// running.enqueue_msg(&stage, [1]);
/// running.resume_receive(&stage).unwrap();
/// running.effect().assert_send(&stage, &output, 2u32);
/// running.resume_send(&stage, &output, 2u32).unwrap();
/// running.effect().assert_receive(&stage);
///
/// running.resume_receive(&output).unwrap();
/// let ext = running.effect().extract_external(&output, &OutputEffect::fake(output.name().clone(), 2u32).0);
/// let result = rt.block_on(ext.run(Resources::default()));
/// running.resume_external(&output, result).unwrap();
/// running.effect().assert_receive(&output);
///
/// // Finally check that the output was received
/// assert_eq!(rx.drain().collect::<Vec<_>>(), vec![2]);
/// ```
pub struct SimulationBuilder {
    stages: BTreeMap<Name, InitStageData>,
    name_counter: usize,
    effect: EffectBox,
    clock: Arc<dyn Clock + Send + Sync>,
    resources: Resources,
    mailbox_size: usize,
    inputs: Inputs,
    trace_buffer: Arc<Mutex<TraceBuffer>>,
}

impl SimulationBuilder {
    pub fn with_mailbox_size(mut self, size: usize) -> Self {
        self.mailbox_size = size;
        self
    }

    pub fn with_trace_buffer(mut self, trace_buffer: Arc<Mutex<TraceBuffer>>) -> Self {
        self.trace_buffer = trace_buffer;
        self
    }

    pub fn replay(self) -> Replay {
        let stages = self
            .stages
            .into_iter()
            .map(|(name, data)| {
                let state = match data.state {
                    InitStageState::Uninitialized => panic!("forgot to wire up stage `{name}`"),
                    InitStageState::Idle(state) => StageState::Idle(state),
                };
                (
                    name.clone(),
                    StageData {
                        name,
                        mailbox: data.mailbox,
                        state,
                        transition: data.transition,
                        waiting: Some(StageEffect::Receive),
                        senders: VecDeque::new(),
                    },
                )
            })
            .collect();
        Replay::new(stages, self.effect)
    }
}

impl Default for SimulationBuilder {
    fn default() -> Self {
        let clock = Arc::new(AtomicU64::new(0));

        Self {
            stages: Default::default(),
            name_counter: Default::default(),
            effect: Default::default(),
            clock,
            resources: Resources::default(),
            mailbox_size: 10,
            inputs: Inputs::new(10),
            // default is a TraceBuffer that drops all messages
            trace_buffer: Arc::new(Mutex::new(TraceBuffer::new(0, 0))),
        }
    }
}

impl super::StageGraph for SimulationBuilder {
    type Running = SimulationRunning;
    type RefAux<Msg, State> = ();

    fn register<Msg, St>(
        &mut self,
        stage: &StageStateRef<Msg, St>,
        stageable: impl Stage<Msg, St> + 'static + Send + Clone,
    ) -> StageRef<Msg>
    where
        Msg: SendData + serde::de::DeserializeOwned,
        St: SendData,
    {
        let name = stage.name();
        let me = StageRef::new(name.clone());
        let self_sender = self.inputs.sender(&me);
        let effects = Effects::new(me, self.effect.clone(), self.clock.clone(), self_sender);
        let initial_state = stageable.initial_state();
        let transition: Transition =
            Box::new(move |state: Box<dyn SendData>, msg: Box<dyn SendData>| {
                let state = state.cast::<St>().expect("internal state type error");
                let msg = msg
                    .cast_deserialize::<Msg>()
                    .expect("internal message type error");
                let effects = effects.clone();
                Box::pin({
                    let stageable_clone = stageable.clone();
                    async move {
                        let fut = stageable_clone.run(*state, msg, effects);
                        Box::new(fut.await) as Box<dyn SendData>
                    }
                })
            });

        if let Some(old) = self.stages.insert(
            name.clone(),
            InitStageData {
                state: InitStageState::Uninitialized,
                mailbox: VecDeque::new(),
                transition,
            },
        ) {
            #[expect(clippy::panic)]
            {
                // names are unique by construction
                panic!("stage {name} already exists with state {:?}", old.state);
            }
        }

        let stage_build_ref = StageBuildRef {
            name: name.clone(),
            network: (),
            _ph: PhantomData,
        };
        self.wire(stage_build_ref, initial_state).without_state()
    }

    fn make_name(&mut self, name: impl AsRef<str>) -> Name {
        // THIS MUST MATCH THE TOKIO BUILDER
        let result = Name::from(&*format!("{}-{}", name.as_ref(), self.name_counter));
        self.name_counter += 1;
        result
    }

    fn wire<Msg, St>(
        &mut self,
        stage: StageBuildRef<Msg, St, Self::RefAux<Msg, St>>,
        state: St,
    ) -> StageStateRef<Msg, St>
    where
        Msg: SendData + serde::de::DeserializeOwned,
        St: SendData,
    {
        let StageBuildRef {
            name,
            network: (),
            _ph,
        } = stage;

        let data = self.stages.get_mut(&name).unwrap();
        data.state = InitStageState::Idle(Box::new(state));

        StageStateRef::new(name)
    }

    fn input<Msg: SendData>(&mut self, stage: impl AsRef<StageRef<Msg>>) -> Sender<Msg> {
        self.inputs.sender(stage.as_ref())
    }

    fn run(self, rt: Handle) -> Self::Running {
        let Self {
            stages: s,
            name_counter: _,
            effect,
            clock,
            resources,
            mailbox_size,
            inputs,
            trace_buffer,
        } = self;
        let mut stages = BTreeMap::new();
        for (
            name,
            InitStageData {
                mailbox,
                state,
                transition,
            },
        ) in s
        {
            let state = match state {
                InitStageState::Uninitialized => panic!("forgot to wire up stage `{name}`"),
                InitStageState::Idle(state) => {
                    trace_buffer.lock().push_state(&name, &state);
                    StageState::Idle(state)
                }
            };
            let data = StageData {
                name: name.clone(),
                mailbox,
                state,
                transition,
                waiting: Some(StageEffect::Receive),
                senders: VecDeque::new(),
            };
            stages.insert(name, data);
        }
        SimulationRunning::new(
            stages,
            inputs,
            effect,
            clock,
            resources,
            mailbox_size,
            rt,
            trace_buffer,
        )
    }

    fn resources(&self) -> &Resources {
        &self.resources
    }
}
