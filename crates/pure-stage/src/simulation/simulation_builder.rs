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

#![expect(clippy::unwrap_used, clippy::panic, clippy::expect_used)]
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
    Clock, Name, Resources, SendData, Sender, StageBuildRef, StageGraph, StageRef,
    effect::{Effects, StageEffect},
    effect_box::EffectBox,
    simulation::{
        inputs::Inputs,
        replay::Replay,
        running::SimulationRunning,
        state::{InitStageData, InitStageState, StageData, StageState, Transition},
    },
    stage_ref::StageStateRef,
    trace_buffer::TraceBuffer,
};

use parking_lot::Mutex;
use rand::SeedableRng;
use rand::rngs::StdRng;
use std::{
    collections::{BTreeMap, VecDeque},
    future::Future,
    marker::PhantomData,
    sync::{Arc, atomic::AtomicU64},
};
use tokio::runtime::Handle;

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
/// use pure_stage::{Resources, StageGraph, simulation::SimulationBuilder, OutputEffect, ExternalEffect};
///
/// let mut network = SimulationBuilder::default();
/// let stage = network.stage("basic", async |(mut state, out), msg: u32, eff| {
///     state += msg;
///     eff.send(&out, state).await;
///     (state, out)
/// });
/// let (output, mut rx) = network.output("output", 10);
/// let stage = network.wire_up(stage, (1u32, output.clone()));
///
/// let rt = tokio::runtime::Runtime::new().unwrap();
/// let mut running = network.run(rt.handle().clone());
///
/// // first check that the stages start out suspended on Receive
/// running.try_effect().unwrap_err().assert_idle();
///
/// // then insert some input and check reaction
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
/// assert_eq!(rx.drain().collect::<Vec<_>>(), vec![2]);
/// ```
pub struct SimulationBuilder {
    stages: BTreeMap<Name, InitStageData>,
    effect: EffectBox,
    clock: Arc<dyn Clock + Send + Sync>,
    resources: Resources,
    mailbox_size: usize,
    inputs: Inputs,
    trace_buffer: Arc<Mutex<TraceBuffer>>,
    rng: Arc<Mutex<StdRng>>,
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

    pub fn with_rng(mut self, rng: Arc<Mutex<StdRng>>) -> Self {
        self.rng = rng;
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
        Replay::new(stages, self.effect, self.trace_buffer)
    }
}

impl Default for SimulationBuilder {
    fn default() -> Self {
        let clock = Arc::new(AtomicU64::new(0));

        Self {
            stages: Default::default(),
            effect: Default::default(),
            clock,
            resources: Resources::default(),
            mailbox_size: 10,
            inputs: Inputs::new(10),
            // default is a TraceBuffer that drops all messages
            trace_buffer: Arc::new(Mutex::new(TraceBuffer::new(0, 0))),
            rng: Arc::new(Mutex::new(StdRng::seed_from_u64(0))),
        }
    }
}

impl StageGraph for SimulationBuilder {
    type Running = SimulationRunning;
    type RefAux<Msg, State> = ();

    fn stage<Msg, St, F, Fut>(
        &mut self,
        name: impl AsRef<str>,
        mut f: F,
    ) -> StageBuildRef<Msg, St, Self::RefAux<Msg, St>>
    where
        F: FnMut(St, Msg, Effects<Msg>) -> Fut + 'static + Send,
        Fut: Future<Output = St> + 'static + Send,
        Msg: SendData + serde::de::DeserializeOwned,
        St: SendData,
    {
        // THIS MUST MATCH THE TOKIO BUILDER
        let name = Name::from(&*format!("{}-{}", name.as_ref(), self.stages.len()));
        let me = StageRef::new(name.clone());
        let self_sender = self.inputs.sender(&me);
        let effects = Effects::new(
            me,
            self.effect.clone(),
            self.clock.clone(),
            self_sender,
            self.resources.clone(),
            self.trace_buffer.clone(),
        );
        let transition: Transition =
            Box::new(move |state: Box<dyn SendData>, msg: Box<dyn SendData>| {
                let state = state.cast::<St>().expect("internal state type error");
                let msg = msg
                    .cast_deserialize::<Msg>()
                    .expect("internal message type error");
                let state = f(*state, msg, effects.clone());
                Box::pin(async move { Box::new(state.await) as Box<dyn SendData> })
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

        StageBuildRef {
            name,
            network: (),
            _ph: PhantomData,
        }
    }

    fn wire_up<Msg: SendData, St: SendData>(
        &mut self,
        stage: crate::StageBuildRef<Msg, St, Self::RefAux<Msg, St>>,
        state: St,
    ) -> StageStateRef<Msg, St> {
        let StageBuildRef {
            name,
            network: (),
            _ph,
        } = stage;

        let data = self.stages.get_mut(&name).unwrap();
        data.state = InitStageState::Idle(Box::new(state));

        StageStateRef::new(name)
    }

    fn preload<Msg: SendData>(
        &mut self,
        stage: impl AsRef<StageRef<Msg>>,
        messages: impl IntoIterator<Item = Msg>,
    ) -> bool {
        let data = self.stages.get_mut(stage.as_ref().name()).unwrap();
        for msg in messages {
            if data.mailbox.len() >= self.mailbox_size {
                return false;
            }
            data.mailbox.push_back(Box::new(msg) as Box<dyn SendData>);
        }
        true
    }

    fn input<Msg: SendData>(&mut self, stage: impl AsRef<StageRef<Msg>>) -> Sender<Msg> {
        self.inputs.sender(stage.as_ref())
    }

    fn run(self, rt: Handle) -> Self::Running {
        let Self {
            stages: s,
            effect,
            clock,
            resources,
            mailbox_size,
            inputs,
            trace_buffer,
            rng,
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
            rng,
        )
    }

    fn resources(&self) -> &Resources {
        &self.resources
    }
}
