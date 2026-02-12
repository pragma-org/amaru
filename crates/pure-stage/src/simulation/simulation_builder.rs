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
    BLACKHOLE_NAME, Clock, Name, Resources, ScheduleIds, SendData, Sender, StageBuildRef,
    StageGraph, StageRef,
    adapter::{Adapter, StageOrAdapter, find_recipient},
    effect::{Effects, StageEffect},
    effect_box::EffectBox,
    simulation::{
        inputs::Inputs,
        random::{EvalStrategy, Fifo},
        replay::Replay,
        running::SimulationRunning,
        state::{InitStageData, InitStageState, StageData, StageState, Transition},
    },
    stage_name,
    stage_ref::StageStateRef,
    trace_buffer::TraceBuffer,
};

use crate::simulation::RandStdRng;
use parking_lot::Mutex;
use rand::SeedableRng;
use rand::prelude::StdRng;
use std::{
    any::Any,
    collections::{BTreeMap, VecDeque},
    future::Future,
    marker::PhantomData,
    sync::{Arc, atomic::AtomicU64},
};

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
/// let mut running = network.run();
/// let rt = tokio::runtime::Runtime::new().unwrap();
///
/// // first check that the stages start out suspended on Receive
/// running.try_effect().unwrap_err().assert_idle();
///
/// // then insert some input and check reaction
/// running.enqueue_msg(&stage, [1]);
/// running.resume_receive(&stage).unwrap();
/// running.effect().assert_send(&stage, &output, 2u32);
/// running.resume_send(&stage, &output, Some(2u32)).unwrap();
/// running.effect().assert_receive(&stage);
///
/// running.resume_receive(&output).unwrap();
/// let ext = running.effect().extract_external::<OutputEffect<u32>>(&output);
/// assert_eq!(&ext.name, output.name());
/// assert_eq!(ext.msg, 2u32);
/// let result = rt.block_on(ext.run(Resources::default()));
/// running.resume_external_box(&output, result).unwrap();
/// running.effect().assert_receive(&output);
///
/// assert_eq!(rx.drain().collect::<Vec<_>>(), vec![2]);
/// ```
pub struct SimulationBuilder {
    stages: BTreeMap<Name, StageOrAdapter<InitStageData>>,
    stage_counter: usize,
    effect: EffectBox,
    clock: Arc<dyn Clock + Send + Sync>,
    resources: Resources,
    schedule_ids: ScheduleIds,
    mailbox_size: usize,
    inputs: Inputs,
    trace_buffer: Arc<Mutex<TraceBuffer>>,
    eval_strategy: Box<dyn EvalStrategy>,
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

    pub fn with_epoch_clock(mut self) -> Self {
        self.clock = Arc::new(AtomicU64::new(0));
        self
    }

    pub fn with_schedule_ids(mut self, schedule_ids: ScheduleIds) -> Self {
        self.schedule_ids = schedule_ids;
        self
    }

    pub fn with_eval_strategy(mut self, eval_strategy: impl EvalStrategy + 'static) -> Self {
        self.eval_strategy = Box::new(eval_strategy);
        self
    }

    pub fn with_seed(self, seed: u64) -> Self {
        self.with_eval_strategy(RandStdRng(StdRng::seed_from_u64(seed)))
    }

    pub fn replay(self) -> Replay {
        let stages = self
            .stages
            .into_iter()
            .filter_map(|(name, data)| {
                let StageOrAdapter::Stage(data) = data else {
                    return None;
                };
                let state = match data.state {
                    InitStageState::Uninitialized => panic!("forgot to wire up stage `{name}`"),
                    InitStageState::Idle(state) => StageState::Idle(state),
                };
                Some((
                    name.clone(),
                    StageData {
                        name,
                        mailbox: data.mailbox,
                        tombstones: VecDeque::new(),
                        state,
                        transition: data.transition,
                        waiting: Some(StageEffect::Receive),
                        senders: VecDeque::new(),
                        supervised_by: BLACKHOLE_NAME.clone(),
                        tombstone: None,
                    },
                ))
            })
            .collect();
        Replay::new(stages, self.effect, self.trace_buffer)
    }

    pub fn run(self) -> SimulationRunning {
        let Self {
            stages: s,
            stage_counter,
            effect,
            clock,
            resources,
            mailbox_size,
            inputs,
            schedule_ids,
            trace_buffer,
            eval_strategy,
        } = self;

        debug_assert_eq!(stage_counter, s.len());

        let mut stages = BTreeMap::new();
        for (name, data) in s {
            let data = match data {
                StageOrAdapter::Stage(data) => data,
                StageOrAdapter::Adapter(adapter) => {
                    stages.insert(adapter.name.clone(), StageOrAdapter::Adapter(adapter));
                    continue;
                }
            };
            let InitStageData {
                mailbox,
                state,
                transition,
            } = data;

            let state = match state {
                InitStageState::Uninitialized => panic!("forgot to wire up stage `{name}`"),
                InitStageState::Idle(state) => {
                    trace_buffer.lock().push_state(&name, &state);
                    StageState::Idle(state)
                }
            };
            let data = StageOrAdapter::Stage(StageData {
                name: name.clone(),
                mailbox,
                tombstones: VecDeque::new(),
                state,
                transition,
                waiting: Some(StageEffect::Receive),
                senders: VecDeque::new(),
                supervised_by: BLACKHOLE_NAME.clone(),
                tombstone: None,
            });
            stages.insert(name, data);
        }
        SimulationRunning::new(
            stages,
            inputs,
            effect,
            clock,
            resources,
            mailbox_size,
            schedule_ids,
            trace_buffer,
            eval_strategy,
        )
    }
}

impl Default for SimulationBuilder {
    fn default() -> Self {
        let clock = Arc::new(AtomicU64::new(0));

        Self {
            stages: Default::default(),
            stage_counter: 0,
            effect: Default::default(),
            clock,
            resources: Resources::default(),
            mailbox_size: 10,
            inputs: Inputs::new(10),
            schedule_ids: ScheduleIds::new(),
            // default is a TraceBuffer that drops all messages
            trace_buffer: Arc::new(Mutex::new(TraceBuffer::new(0, 0))),
            eval_strategy: Box::new(Fifo),
        }
    }
}

impl StageGraph for SimulationBuilder {
    fn stage<Msg, St, F, Fut>(
        &mut self,
        name: impl AsRef<str>,
        mut f: F,
    ) -> StageBuildRef<Msg, St, Box<dyn Any + Send>>
    where
        F: FnMut(St, Msg, Effects<Msg>) -> Fut + 'static + Send,
        Fut: Future<Output = St> + 'static + Send,
        Msg: SendData + serde::de::DeserializeOwned,
        St: SendData,
    {
        // THIS MUST MATCH THE TOKIO BUILDER
        let name = stage_name(&mut self.stage_counter, name.as_ref());
        let me = StageRef::new(name.clone());
        let effects = Effects::new(
            me,
            self.effect.clone(),
            self.clock.clone(),
            self.resources.clone(),
            self.schedule_ids.clone(),
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
            StageOrAdapter::Stage(InitStageData {
                state: InitStageState::Uninitialized,
                mailbox: VecDeque::new(),
                transition,
            }),
        ) {
            #[expect(clippy::panic)]
            {
                // names are unique by construction
                panic!("stage {name} already exists with state {:?}", old);
            }
        }

        StageBuildRef {
            name,
            network: Box::new(()),
            _ph: PhantomData,
        }
    }

    fn wire_up<Msg: SendData, St: SendData>(
        &mut self,
        stage: crate::StageBuildRef<Msg, St, Box<dyn Any + Send>>,
        state: St,
    ) -> StageStateRef<Msg, St> {
        let StageBuildRef {
            name,
            network: _,
            _ph,
        } = stage;

        let StageOrAdapter::Stage(data) = self.stages.get_mut(&name).unwrap() else {
            panic!("stage {name} is an adapter");
        };
        data.state = InitStageState::Idle(Box::new(state));

        StageStateRef::new(name)
    }

    fn contramap<Original: SendData, Mapped: SendData>(
        &mut self,
        stage_ref: impl AsRef<StageRef<Original>>,
        new_name: impl AsRef<str>,
        transform: impl Fn(Mapped) -> Original + 'static + Send,
    ) -> StageRef<Mapped> {
        let target = stage_ref.as_ref().name().clone();
        let new_name = stage_name(&mut self.stage_counter, new_name.as_ref());
        let adapter = Adapter::new(new_name.clone(), target, transform);
        self.stages
            .insert(adapter.name.clone(), StageOrAdapter::Adapter(adapter));
        StageRef::new(new_name)
    }

    fn preload<Msg: SendData>(
        &mut self,
        stage: impl AsRef<StageRef<Msg>>,
        messages: impl IntoIterator<Item = Msg>,
    ) -> Result<(), Box<dyn SendData>> {
        for msg in messages {
            deliver_message(
                &mut self.stages,
                self.mailbox_size,
                stage.as_ref().name().clone(),
                Box::new(msg) as Box<dyn SendData>,
            )?;
        }
        Ok(())
    }

    fn input<Msg: SendData>(&mut self, stage: impl AsRef<StageRef<Msg>>) -> Sender<Msg> {
        self.inputs.sender(stage.as_ref())
    }

    fn resources(&self) -> &Resources {
        &self.resources
    }
}

fn deliver_message(
    stages: &mut BTreeMap<Name, StageOrAdapter<InitStageData>>,
    mailbox_size: usize,
    name: Name,
    msg: Box<dyn SendData>,
) -> Result<(), Box<dyn SendData>> {
    let Some((data, msg)) = find_recipient(stages, name.clone(), Some(msg)) else {
        return Err(Box::new(format!("stage {name} does not exist")));
    };
    if data.mailbox.len() >= mailbox_size {
        return Err(msg);
    }
    data.mailbox.push_back(msg);
    Ok(())
}
