#![expect(clippy::disallowed_types)]
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

use anyhow::{Context as _, ensure};
use cbor4ii::serde::from_slice;
use std::{collections::HashMap, mem::replace, sync::Arc};

use crate::{
    Effect, Instant, Name, SendData,
    effect::ScheduleIds,
    effect_box::EffectBox,
    serde::to_cbor,
    simulation::{
        running::poll_stage,
        state::{StageData, StageState},
    },
    time::EPOCH,
    trace_buffer::{TraceBuffer, TraceEntry},
};
use parking_lot::Mutex;

/// A replay of a simulation.
///
/// This is used to replay a simulation from a trace.
pub struct Replay {
    stages: HashMap<Name, StageData>,
    effect: EffectBox,
    trace_buffer: Arc<Mutex<TraceBuffer>>,
    schedule_ids_counter: ScheduleIds,
    pending_suspend: HashMap<Name, Effect>,
    latest_state: HashMap<Name, Box<dyn SendData>>,
    clock: Instant,
}

impl Replay {
    pub(crate) fn new(
        stages: HashMap<Name, StageData>,
        effect: EffectBox,
        trace_buffer: Arc<Mutex<TraceBuffer>>,
    ) -> Self {
        Self {
            stages,
            effect,
            trace_buffer,
            schedule_ids_counter: ScheduleIds::default(),
            pending_suspend: HashMap::new(),
            latest_state: HashMap::new(),
            clock: *EPOCH,
        }
    }

    pub fn run_trace(&mut self, trace: Vec<TraceEntry>) -> anyhow::Result<()> {
        let mut trace = trace.into_iter();
        let mut idx = 0;
        loop {
            let Some(entry) = trace.next() else {
                break;
            };

            match entry {
                TraceEntry::Suspend(effect) => {
                    let name = effect.at_stage();
                    let expected = self.pending_suspend.remove(name);
                    ensure!(
                        expected.as_ref() == Some(&effect),
                        "idx {}: stage {} suspended with effect {:?},\nbut expected {:?}",
                        idx,
                        name,
                        effect,
                        expected
                    );
                }
                TraceEntry::Resume {
                    stage, response, ..
                } => {
                    let data = self
                        .stages
                        .get_mut(&stage)
                        .context(format!("idx {}: stage {} not found", idx, stage))?;

                    let remaining = trace.as_slice().len();
                    self.trace_buffer.lock().set_fetch_replay(trace);

                    let effect = poll_stage(
                        &self.trace_buffer,
                        &self.schedule_ids_counter,
                        data,
                        stage,
                        response,
                        &self.effect,
                        self.clock,
                    );

                    trace = self
                        .trace_buffer
                        .lock()
                        .take_fetch_replay()
                        .ok_or_else(|| anyhow::anyhow!("idx {}: no fetch replay found", idx))?;
                    idx += remaining - trace.as_slice().len();

                    // the effect we'll see in the log (see TraceEntry::Suspend above)
                    // is the deserialized version, i.e. generic encoding;
                    // so we need to produce the generic encoding here; unfortunately, there
                    // is no direct serialization to cbor4ii `Value`, so (de)serialize
                    let effect: Effect =
                        from_slice(&to_cbor(&effect)).expect("internal replay error");
                    self.pending_suspend
                        .insert(effect.at_stage().clone(), effect);
                }
                TraceEntry::Clock(instant) => {
                    self.clock = instant;
                }
                TraceEntry::Input { stage, input } => {
                    let data = self
                        .stages
                        .get_mut(&stage)
                        .context(format!("idx {}: stage {} not found", idx, stage))?;
                    match &mut data.state {
                        StageState::Idle(state) => {
                            let state = replace(state, Box::new(()));
                            data.state = StageState::Running((data.transition)(state, input));
                        }
                        _ => anyhow::bail!("idx {}: stage {} is not idle", idx, stage),
                    }
                }
                TraceEntry::State { stage, state } => {
                    let data = self
                        .stages
                        .get_mut(&stage)
                        .context(format!("idx {}: stage {} not found", idx, stage))?;
                    match &data.state {
                        StageState::Idle(s) => {
                            let state = s.deserialize_value(&*state)?;
                            ensure!(
                                **s == *state,
                                "idx {}: stage {} state mismatch: {:?} != {:?}",
                                idx,
                                stage,
                                &**s,
                                &*state
                            );
                            self.latest_state.insert(stage, state);
                        }
                        StageState::Running(_) => anyhow::bail!(
                            "idx {}: stage {} is running while it should be in state {:?}",
                            idx,
                            stage,
                            &*state
                        ),
                        StageState::Terminating => anyhow::bail!(
                            "idx {}: stage {} is terminating while it should be in state {:?}",
                            idx,
                            stage,
                            &*state
                        ),
                    }
                }
                TraceEntry::Terminated { stage, reason: _ } => {
                    self.latest_state.remove(&stage);
                }
            }
            idx += 1;
        }
        Ok(())
    }

    pub fn pending_suspend(&self) -> &HashMap<Name, Effect> {
        &self.pending_suspend
    }

    pub fn latest_state(&self, stage: &Name) -> Option<&dyn SendData> {
        self.latest_state.get(stage).map(|s| &**s)
    }

    pub fn is_running(&self, stage: &Name) -> bool {
        matches!(
            self.stages.get(stage).unwrap().state,
            StageState::Running(_)
        )
    }

    pub fn is_terminating(&self, stage: &Name) -> bool {
        matches!(
            self.stages.get(stage).unwrap().state,
            StageState::Terminating
        )
    }

    pub fn is_idle(&self, stage: &Name) -> bool {
        matches!(self.stages.get(stage).unwrap().state, StageState::Idle(_))
    }

    pub fn clock(&self) -> Instant {
        self.clock
    }
}
