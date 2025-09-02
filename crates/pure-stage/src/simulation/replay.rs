#![allow(clippy::disallowed_types)]
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

use crate::{
    Effect, Instant, StageName, SendData,
    serde::to_cbor,
    simulation::{
        EffectBox,
        running::poll_stage,
        state::{StageData, StageState},
    },
    time::EPOCH,
    trace_buffer::{TraceBuffer, TraceEntry},
};
use anyhow::{Context as _, ensure};
use cbor4ii::serde::from_slice;
use std::{collections::HashMap, mem::replace};

/// A replay of a simulation.
///
/// This is used to replay a simulation from a trace.
pub struct Replay {
    stages: HashMap<StageName, StageData>,
    effect: EffectBox,
    pending_suspend: HashMap<StageName, Effect>,
    latest_state: HashMap<StageName, Box<dyn SendData>>,
    clock: Instant,
}

impl Replay {
    pub(crate) fn new(stages: HashMap<StageName, StageData>, effect: EffectBox) -> Self {
        Self {
            stages,
            effect,
            pending_suspend: HashMap::new(),
            latest_state: HashMap::new(),
            clock: *EPOCH,
        }
    }

    pub fn run_trace(&mut self, trace: Vec<TraceEntry>) -> anyhow::Result<()> {
        let mut trace_buffer = TraceBuffer::new(0, 0);

        for (idx, entry) in trace.into_iter().enumerate() {
            match entry {
                TraceEntry::Suspend(effect) => {
                    let name = effect.at_stage();
                    let expected = self.pending_suspend.remove(name);
                    ensure!(
                        expected.as_ref() == Some(&effect),
                        "idx {}: stage {} suspended with effect {:?}, but expected {:?}",
                        idx,
                        name,
                        effect,
                        expected
                    );
                }
                TraceEntry::Resume { stage, response } => {
                    let data = self
                        .stages
                        .get_mut(&stage)
                        .context(format!("idx {}: stage {} not found", idx, stage))?;
                    let effect = poll_stage(&mut trace_buffer, data, stage, response, &self.effect);
                    // the effect we'll see in the log is the deserialized version, i.e. generic encoding
                    // so we need to store the generic encoding here
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
                        StageState::Failed(error) => anyhow::bail!(
                            "idx {}: stage {} is failed while it should be in state {:?}: {}",
                            idx,
                            stage,
                            &*state,
                            error
                        ),
                    }
                }
            }
        }
        Ok(())
    }

    pub fn pending_suspend(&self) -> &HashMap<StageName, Effect> {
        &self.pending_suspend
    }

    pub fn latest_state(&self, stage: &StageName) -> Option<&dyn SendData> {
        self.latest_state.get(stage).map(|s| &**s)
    }

    pub fn is_running(&self, stage: &StageName) -> bool {
        matches!(
            self.stages.get(stage).unwrap().state,
            StageState::Running(_)
        )
    }

    pub fn is_failed(&self, stage: &StageName) -> bool {
        matches!(self.stages.get(stage).unwrap().state, StageState::Failed(_))
    }

    pub fn is_idle(&self, stage: &StageName) -> bool {
        matches!(self.stages.get(stage).unwrap().state, StageState::Idle(_))
    }

    pub fn get_failure(&self, stage: &StageName) -> Option<&str> {
        self.stages.get(stage).and_then(|s| match &s.state {
            StageState::Idle(_) | StageState::Running(_) => None,
            StageState::Failed(error) => Some(&**error),
        })
    }

    pub fn clock(&self) -> Instant {
        self.clock
    }
}
