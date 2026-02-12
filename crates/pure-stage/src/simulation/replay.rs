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

use crate::{
    Effect, Instant, Name, SendData, StageResponse,
    effect::{CanSupervise, ScheduleIds, StageEffect},
    effect_box::EffectBox,
    serde::{SendDataValue, to_cbor},
    simulation::{
        running::poll_stage,
        state::{StageData, StageState},
    },
    time::EPOCH,
    trace_buffer::{TraceBuffer, TraceEntry},
};
use anyhow::{Context as _, ensure};
use cbor4ii::serde::from_slice;
use parking_lot::Mutex;
use std::{
    collections::{HashMap, VecDeque},
    mem::replace,
    sync::Arc,
};

/// A replay of a simulation.
///
/// This allows a user to provide a list of trace entries, run them one by one and
/// reproduce the state of the system (the stages) to make it similar to what it was during the
/// original execution.
///
pub struct Replay {
    stages: HashMap<Name, StageData>,
    effect: EffectBox,
    trace_buffer: Arc<Mutex<TraceBuffer>>,
    schedule_ids: ScheduleIds,
    pending_suspend: HashMap<Name, Effect>,
    latest_state: HashMap<Name, Box<dyn SendData>>,
    clock: Instant,
}

impl Replay {
    pub(crate) fn new(
        stages: HashMap<Name, StageData>,
        effect: EffectBox,
        trace_buffer: Arc<Mutex<TraceBuffer>>,
        schedule_ids: ScheduleIds,
    ) -> Self {
        Self {
            stages,
            effect,
            trace_buffer,
            schedule_ids,
            pending_suspend: HashMap::new(),
            latest_state: HashMap::new(),
            clock: *EPOCH,
        }
    }

    /// Run all the entries of a given trace and update the stages states.
    pub fn run_trace(&mut self, trace: Vec<TraceEntry>) -> anyhow::Result<()> {
        let mut trace = trace.into_iter();
        let mut idx = 0;
        loop {
            let Some(entry) = trace.next() else {
                break;
            };

            match entry {
                TraceEntry::Suspend(effect) => {
                    let actual = deserialize_effect(effect)?;
                    let name = actual.at_stage();
                    let expected = self.pending_suspend.remove(name);
                    ensure!(
                        expected.as_ref() == Some(&actual),
                        "idx {}: stage {} suspended with effect {:?},\nbut expected {:?}",
                        idx,
                        name,
                        actual,
                        expected,
                    );
                    // handle dynamic stages
                    match actual {
                        Effect::AddStage { at_stage, name } => {
                            self.handle_add_stage(at_stage, name, idx)?;
                        }
                        Effect::WireStage {
                            at_stage,
                            name,
                            initial_state,
                            tombstone,
                            ..
                        } => {
                            self.handle_wire_stage(at_stage, name, initial_state, tombstone, idx)?;
                        }
                        _ => {}
                    }
                }
                TraceEntry::Resume {
                    stage, response, ..
                } => {
                    let data = self
                        .stages
                        .get_mut(&stage)
                        .context(format!("idx {}: stage {} not found", idx, stage))?;

                    let response = materialize_stage_response(response)?;
                    let remaining = trace.as_slice().len();
                    self.trace_buffer.lock().set_fetch_replay(trace);

                    let effect = poll_stage(
                        &self.trace_buffer,
                        &self.schedule_ids,
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
                    let input = deserialize_send_data_value(input)?;
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

    /// Check that we were waiting for an AddStage effect (and the correct one)
    fn handle_add_stage(&mut self, at_stage: Name, name: Name, idx: usize) -> anyhow::Result<()> {
        match self.get_waiting_effect(&at_stage, "add stage", idx)? {
            StageEffect::AddStage(expected_name) => {
                check_stage_name(name.clone(), expected_name, "add stage", idx)
            }
            other => anyhow::bail!(
                "idx {}: stage {} was waiting for {:?} when handling add stage",
                idx,
                at_stage,
                other
            ),
        }
    }

    /// Check that we were waiting for a WireStage effect and register the newly created stage.
    fn handle_wire_stage(
        &mut self,
        at_stage: Name,
        name: Name,
        initial_state: Box<dyn SendData>,
        tombstone: Box<dyn SendData>,
        idx: usize,
    ) -> anyhow::Result<()> {
        match self.get_waiting_effect(&at_stage, "wire stage", idx)? {
            StageEffect::WireStage(expected_name, transition, _, _) => {
                check_stage_name(name.clone(), expected_name, "wire stage", idx)?;
                let initial_state = deserialize_send_data_value(initial_state)?;
                let tombstone = deserialize_send_data_value(tombstone)?;

                let transition = (transition.into_inner())(self.effect.clone());
                let tombstone = tombstone.try_cast::<CanSupervise>().err();

                ensure!(
                    self.stages
                        .insert(
                            name.clone(),
                            StageData {
                                name: name.clone(),
                                mailbox: VecDeque::new(),
                                tombstones: VecDeque::new(),
                                state: StageState::Idle(initial_state),
                                transition,
                                waiting: Some(StageEffect::Receive),
                                senders: VecDeque::new(),
                                supervised_by: at_stage,
                                tombstone,
                            },
                        )
                        .is_none(),
                    "idx {}: stage {} already exists while wiring stage",
                    idx,
                    name
                );
                Ok(())
            }
            other => {
                anyhow::bail!(
                    "idx {}: stage {} was waiting for {:?} when handling wire stage",
                    idx,
                    at_stage,
                    other
                );
            }
        }
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

    /// Get the effect we were waiting for on a stage, and remove it from the stage data.
    fn get_waiting_effect(
        &mut self,
        at_stage: &Name,
        effect_name: &str,
        idx: usize,
    ) -> anyhow::Result<StageEffect<()>> {
        let data = self.stages.get_mut(at_stage).with_context(|| {
            format!(
                "idx {}: stage {} not found while handling {} effect",
                idx, at_stage, effect_name
            )
        })?;
        data.waiting.take().with_context(|| {
            format!(
                "idx {}: stage {} was not waiting for any effect while handling add stage",
                idx, at_stage
            )
        })
    }
}

/// Checks if the actual stage name matches the expected stage name.
fn check_stage_name(
    actual_name: Name,
    expected_name: Name,
    effect_name: &str,
    idx: usize,
) -> anyhow::Result<()> {
    ensure!(
        expected_name == actual_name,
        "idx {}: {effect_name} name mismatch: expected {:?}, got {:?}",
        idx,
        expected_name,
        actual_name
    );
    Ok(())
}

/// If this dyn SendData has been serialized as a SendDataValue,
/// Retrieve its cbor representation and deserialize it as the original dyn SendData
/// in order to be able to do equality checks on it.
fn deserialize_send_data_value(data: Box<dyn SendData>) -> anyhow::Result<Box<dyn SendData>> {
    if data.is::<SendDataValue>() {
        Ok(From::<SendDataValue>::from(*data.cast::<SendDataValue>()?))
    } else {
        Ok(data)
    }
}

/// Recover the actual dyn SendData values in a StageResponse.
fn materialize_stage_response(response: StageResponse) -> anyhow::Result<StageResponse> {
    match response {
        StageResponse::CallResponse(data) => Ok(StageResponse::CallResponse(
            deserialize_send_data_value(data)?,
        )),
        StageResponse::ExternalResponse(data) => Ok(StageResponse::ExternalResponse(
            deserialize_send_data_value(data)?,
        )),
        other => Ok(other),
    }
}

/// Recover the actual dyn SendData values in an effect.
fn deserialize_effect(effect: Effect) -> anyhow::Result<Effect> {
    match effect {
        Effect::Send { from, to, msg } => Ok(Effect::Send {
            from,
            to,
            msg: deserialize_send_data_value(msg)?,
        }),
        Effect::Call {
            from,
            to,
            duration,
            msg,
        } => Ok(Effect::Call {
            from,
            to,
            duration,
            msg: deserialize_send_data_value(msg)?,
        }),
        Effect::Schedule { at_stage, msg, id } => Ok(Effect::Schedule {
            at_stage,
            msg: deserialize_send_data_value(msg)?,
            id,
        }),
        Effect::WireStage {
            at_stage,
            name,
            initial_state,
            tombstone,
        } => Ok(Effect::WireStage {
            at_stage,
            name,
            initial_state: deserialize_send_data_value(initial_state)?,
            tombstone: deserialize_send_data_value(tombstone)?,
        }),
        other => Ok(other),
    }
}
