#![expect(clippy::borrowed_box)]
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

//! This module contains the [`TraceBuffer`] type, which is used to record the trace of a simulation.

use crate::effect::StageEffect;
use crate::{Effect, Instant, Name, SendData, effect::StageResponse, serde::to_cbor};
use crate::{ExternalEffect, ScheduleId};
use cbor4ii::serde::from_slice;
use parking_lot::Mutex;
use std::borrow::Borrow;
use std::fmt::{Debug, Display, Formatter};
use std::time::Duration;
use std::{collections::VecDeque, sync::Arc};

/// A buffer for recording the trace of a simulation.
///
/// The buffer has a bounded size and will drop the oldest entries when it is full.
/// Attempting to push an entry that would exceed the size will try to free up space, but it will
/// retain at least `min_entries`; if this does not suffice, the new entry will be dropped.
///
/// Each message in the buffer is a CBOR-encoded tuple of `(Instant, TraceEntry)`.
#[derive(Default)]
pub struct TraceBuffer {
    messages: VecDeque<Vec<u8>>,
    min_entries: usize,
    max_size: usize,
    used_size: usize,
    dropped_messages: usize,
    fetch_replay: Option<std::vec::IntoIter<TraceEntry>>,
}

/// An owned entry can be serialized and deserialized.
///
/// However, we need the entries in the continuation of the running program and we cannot clone,
/// so there is a variant of this enum with the very same structure the captures only references
/// to work around this problem: [`TraceEntryRef`].
#[derive(PartialEq, serde::Serialize, serde::Deserialize)]
pub enum TraceEntry {
    Suspend(Effect),
    Resume {
        stage: Name,
        response: StageResponse,
    },
    Clock(Instant),
    Input {
        stage: Name,
        #[serde(with = "crate::serde::serialize_send_data")]
        input: Box<dyn SendData>,
    },
    State {
        stage: Name,
        #[serde(with = "crate::serde::serialize_send_data")]
        state: Box<dyn SendData>,
    },
    Terminated {
        stage: Name,
        reason: TerminationReason,
    },
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum TerminationReason {
    Voluntary,
    Supervision(Name),
    Aborted,
}

impl TraceEntry {
    pub fn to_json(&self) -> serde_json::Value {
        match &self {
            TraceEntry::Suspend(effect) => serde_json::json!({
                "type": "suspend",
                "effect": effect.to_json(),
            }),
            TraceEntry::Resume { stage, response } => serde_json::json!({
                "type": "resume",
                "stage": stage,
                "response": response,
            }),
            TraceEntry::Clock(instant) => serde_json::json!({
            "type": "clock",
            "instant": instant,
            }),
            TraceEntry::Input { stage, input } => serde_json::json!({
            "type": "input",
            "stage": stage,
            "input": input.to_string(),
            }),
            TraceEntry::State { stage, state } => serde_json::json!({
            "type": "state",
            "stage": stage,
            "state": state.to_string(),
            }),
            TraceEntry::Terminated { stage, reason } => serde_json::json!({
            "type": "terminated",
            "stage": stage,
            "reason": reason,
            }),
        }
    }
}

impl Debug for TraceEntry {
    /// This debug instance does not output the runnable field in the Resume case.
    /// That field is only useful for display purposes.
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            TraceEntry::Suspend(effect) => f.debug_tuple("Suspend").field(&effect).finish(),
            TraceEntry::Resume {
                stage, response, ..
            } => f
                .debug_struct("Resume")
                .field("stage", stage)
                .field("response", response)
                .finish(),
            TraceEntry::Clock(instant) => f.debug_tuple("Clock").field(instant).finish(),
            TraceEntry::Input { stage, input } => f
                .debug_struct("Input")
                .field("stage", stage)
                .field("input", input)
                .finish(),
            TraceEntry::State { stage, state } => f
                .debug_struct("State")
                .field("stage", stage)
                .field("state", state)
                .finish(),
            TraceEntry::Terminated { stage, reason } => f
                .debug_struct("Terminated")
                .field("stage", stage)
                .field("reason", reason)
                .finish(),
        }
    }
}

impl Display for TraceEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TraceEntry::Suspend(effect) => write!(f, "suspend {stage}", stage = effect.at_stage()),
            TraceEntry::Resume { stage, response } => {
                write!(
                    f,
                    "resume {stage}{response}",
                    stage = stage.as_str(),
                    response = match response {
                        StageResponse::Unit => "".to_string(),
                        other @ StageResponse::ClockResponse(_)
                        | other @ StageResponse::WaitResponse(_)
                        | other @ StageResponse::CallResponse(_)
                        | other @ StageResponse::CancelScheduleResponse(_)
                        | other @ StageResponse::ExternalResponse(_)
                        | other @ StageResponse::AddStageResponse(_)
                        | other @ StageResponse::ContramapResponse(_) => format!(" -> {other}"),
                    },
                )
            }
            TraceEntry::Clock(instant) => write!(f, "clock {instant}"),
            TraceEntry::Input { stage, input } => {
                write!(
                    f,
                    "input {stage} -> {input}",
                    stage = stage.as_str(),
                    input = input.as_send_data_value().borrow()
                )
            }
            TraceEntry::State { stage, state } => {
                write!(
                    f,
                    "state {stage} -> {state}",
                    stage = stage.as_str(),
                    state = state.as_send_data_value().borrow()
                )
            }
            TraceEntry::Terminated { stage, reason } => {
                write!(f, "terminated {stage} {reason:?}")
            }
        }
    }
}

impl TraceEntry {
    /// Construct a suspend entry.
    pub fn suspend(effect: Effect) -> Self {
        Self::Suspend(effect)
    }

    /// Construct a resume entry.
    pub fn resume(stage: impl AsRef<str>, response: StageResponse) -> Self {
        Self::Resume {
            stage: Name::from(stage.as_ref()),
            response,
        }
    }

    /// Construct a clock entry.
    pub fn clock(instant: Instant) -> Self {
        Self::Clock(instant)
    }

    /// Construct an input entry.
    pub fn input(stage: impl AsRef<str>, input: Box<dyn SendData>) -> Self {
        Self::Input {
            stage: Name::from(stage.as_ref()),
            input,
        }
    }

    /// Construct a state entry.
    pub fn state(stage: impl AsRef<str>, state: Box<dyn SendData>) -> Self {
        Self::State {
            stage: Name::from(stage.as_ref()),
            state,
        }
    }

    /// Construct a terminated entry.
    pub fn terminated(stage: impl AsRef<str>, reason: TerminationReason) -> Self {
        Self::Terminated {
            stage: Name::from(stage.as_ref()),
            reason,
        }
    }

    pub fn at_stage<'a, 'b: 'a>(&'b self) -> Option<&'a Name> {
        match self {
            TraceEntry::Suspend(effect) => Some(effect.at_stage()),
            TraceEntry::Resume { stage, .. } => Some(stage),
            TraceEntry::Clock(..) => None,
            TraceEntry::Input { stage, .. } => Some(stage),
            TraceEntry::State { stage, .. } => Some(stage),
            TraceEntry::Terminated { stage, .. } => Some(stage),
        }
    }
}

/// A non-owning variant of [`TraceEntry`] that allows serializing an entry without consuming it.
///
/// This structure cannot be deserialized, use the owned version for that.
#[derive(Debug, PartialEq, serde::Serialize)]
enum TraceEntryRef<'a> {
    Suspend(&'a Effect),
    Resume {
        stage: &'a Name,
        response: &'a StageResponse,
    },
    Clock(Instant),
    Input {
        stage: &'a Name,
        input: &'a Box<dyn SendData>,
    },
    State {
        stage: &'a Name,
        state: &'a Box<dyn SendData>,
    },
    Terminated {
        stage: &'a Name,
        reason: TerminationReasonRef<'a>,
    },
}

#[derive(Debug, PartialEq, serde::Serialize)]
enum TerminationReasonRef<'a> {
    Voluntary,
    Supervision(&'a Name),
    Aborted,
}

/// Helper struct that has the same serialization format as TraceEntry but doesn’t require owned effect data.
#[derive(serde::Serialize)]
enum TraceEntryRefRef<'a> {
    Suspend(EffectRef<'a>),
    Resume {
        stage: &'a Name,
        response: StageResponseRef<'a>,
    },
}

/// Helper struct that has the same serialization format as Effect but doesn’t require owned effect data.
#[derive(serde::Serialize)]
enum EffectRef<'a> {
    Receive {
        at_stage: &'a Name,
    },
    Send {
        from: &'a Name,
        to: &'a Name,
        call: bool,
        msg: &'a dyn SendData,
    },
    Call {
        from: &'a Name,
        to: &'a Name,
        duration: Duration,
        msg: &'a dyn SendData,
    },
    Clock {
        at_stage: &'a Name,
    },
    Wait {
        at_stage: &'a Name,
        duration: Duration,
    },
    Schedule {
        at_stage: &'a Name,
        msg: &'a dyn SendData,
        id: ScheduleId,
    },
    CancelSchedule {
        at_stage: &'a Name,
        id: ScheduleId,
    },
    External {
        at_stage: &'a Name,
        effect: &'a dyn crate::ExternalEffect,
    },
    Terminate {
        at_stage: &'a Name,
    },
    AddStage {
        at_stage: &'a Name,
        name: &'a Name,
    },
    WireStage {
        at_stage: &'a Name,
        name: &'a Name,
        initial_state: &'a dyn SendData,
        tombstone: &'a dyn SendData,
    },
    Contramap {
        at_stage: &'a Name,
        original: &'a Name,
        new_name: &'a Name,
    },
}

impl<'a> EffectRef<'a> {
    fn from(at_stage: &'a Name, effect: &'a StageEffect<Box<dyn SendData>>) -> Option<Self> {
        Some(match effect {
            StageEffect::Receive => EffectRef::Receive { at_stage },
            StageEffect::Send(to, call, msg) => EffectRef::Send {
                from: at_stage,
                to,
                call: call.is_some(),
                msg: &**msg,
            },
            StageEffect::Call(..) => return None,
            StageEffect::Clock => EffectRef::Clock { at_stage },
            StageEffect::Wait(duration) => EffectRef::Wait {
                at_stage,
                duration: *duration,
            },
            StageEffect::Schedule(msg, id) => EffectRef::Schedule {
                at_stage,
                msg: &**msg,
                id: *id,
            },
            StageEffect::CancelSchedule(id) => EffectRef::CancelSchedule { at_stage, id: *id },
            StageEffect::External(effect) => EffectRef::External {
                at_stage,
                effect: &**effect,
            },
            StageEffect::Terminate => EffectRef::Terminate { at_stage },
            StageEffect::AddStage(name) => EffectRef::AddStage { at_stage, name },
            StageEffect::WireStage(name, _transition, initial_state, tombstone) => {
                EffectRef::WireStage {
                    at_stage,
                    name,
                    initial_state: &**initial_state,
                    tombstone: &**tombstone,
                }
            }
            StageEffect::Contramap {
                original,
                new_name,
                transform: _,
            } => EffectRef::Contramap {
                at_stage,
                original,
                new_name,
            },
        })
    }
}

/// Helper struct that has the same serialization format as StageResponse but doesn’t require owned response data.
#[derive(serde::Serialize)]
enum StageResponseRef<'a> {
    ExternalResponse(&'a dyn SendData),
}

impl TraceBuffer {
    /// Create a new shareable trace buffer.
    ///
    /// This is used to retain a reference to the trace buffer while the simulation is running.
    pub fn new_shared(min_entries: usize, max_size: usize) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self::new(min_entries, max_size)))
    }

    /// Create a new trace buffer with the given size limits.
    ///
    /// Set `min_entries` and `max_size` to zero to obtain a buffer that will drop all entries
    /// without allocating memory or otherwise wasting much time.
    pub fn new(min_entries: usize, max_size: usize) -> Self {
        Self {
            messages: VecDeque::new(),
            min_entries,
            max_size,
            used_size: 0,
            dropped_messages: 0,
            fetch_replay: None,
        }
    }

    /// Push an effect to the trace buffer.
    pub fn push_suspend(&mut self, effect: &Effect) {
        self.push(TraceEntryRef::Suspend(effect));
    }

    pub fn push_suspend_external(&mut self, at_stage: &Name, effect: &dyn crate::ExternalEffect) {
        self.push(TraceEntryRefRef::Suspend(EffectRef::External {
            at_stage,
            effect,
        }));
    }

    pub fn push_suspend_call(
        &mut self,
        at_stage: &Name,
        to: &Name,
        duration: Duration,
        msg: &dyn SendData,
    ) {
        self.push(TraceEntryRefRef::Suspend(EffectRef::Call {
            from: at_stage,
            to,
            duration,
            msg,
        }));
    }

    pub fn push_suspend_ref(&mut self, at_stage: &Name, effect: &StageEffect<Box<dyn SendData>>) {
        if let Some(effect) = EffectRef::from(at_stage, effect) {
            self.push(TraceEntryRefRef::Suspend(effect));
        }
    }

    /// Push a resume event to the trace buffer.
    pub fn push_resume(&mut self, stage: &Name, response: &StageResponse) {
        self.push(TraceEntryRef::Resume { stage, response });
    }

    pub fn push_resume_external(&mut self, stage: &Name, response: &dyn SendData) {
        self.push(TraceEntryRefRef::Resume {
            stage,
            response: StageResponseRef::ExternalResponse(response),
        });
    }

    /// Push a clock update to the trace buffer.
    pub fn push_clock(&mut self, instant: Instant) {
        self.push(TraceEntryRef::Clock(instant));
    }

    /// Push a receive event to the trace buffer.
    pub fn push_input(&mut self, stage: &Name, input: &Box<dyn SendData>) {
        self.push(TraceEntryRef::Input { stage, input });
    }

    /// Push a state update to the trace buffer.
    ///
    /// This happens every time polling a stage yields a `Poll::Ready(Ok(...))`, i.e. as soon as the next
    /// stage state has been computed.
    pub fn push_state(&mut self, stage: &Name, state: &Box<dyn SendData>) {
        self.push(TraceEntryRef::State { stage, state });
    }

    pub fn push_terminated_voluntary(&mut self, stage: &Name) {
        self.push(TraceEntryRef::Terminated {
            stage,
            reason: TerminationReasonRef::Voluntary,
        });
    }

    pub fn push_terminated_supervision(&mut self, stage: &Name, child: &Name) {
        self.push(TraceEntryRef::Terminated {
            stage,
            reason: TerminationReasonRef::Supervision(child),
        });
    }

    pub fn push_terminated_aborted(&mut self, stage: &Name) {
        self.push(TraceEntryRef::Terminated {
            stage,
            reason: TerminationReasonRef::Aborted,
        });
    }

    fn push<T: serde::Serialize>(&mut self, msg: T) {
        let msg = to_cbor(&(Instant::now(), msg));

        if self.max_size == 0 {
            return;
        }

        self.used_size += msg.len();

        #[expect(clippy::expect_used)]
        while self.used_size > self.max_size && self.messages.len() > self.min_entries {
            self.used_size -= self
                .messages
                .pop_front()
                .expect("messages is definitely not empty")
                .len();
            self.dropped_messages += 1;
        }

        // the loop above keeps the number of entries at least at min_entries, so if we can
        // push a new one later we can remove one more
        let one_more = self.messages.front().map(|m| m.len()).unwrap_or(0);
        if self.used_size > self.max_size && self.used_size - one_more <= self.max_size {
            self.used_size -= one_more;
            self.messages.pop_front();
        }

        if self.used_size > self.max_size {
            self.used_size -= msg.len();
            self.dropped_messages += 1;
            return;
        }

        self.messages.push_back(msg);
    }

    /// Iterate over the entries in the trace buffer.
    pub fn iter(&self) -> impl Iterator<Item = &[u8]> {
        self.messages.iter().map(|m| m.as_slice())
    }

    /// Take the entries from the trace buffer, leaving it empty.
    pub fn take(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.messages).into()
    }

    /// Hydrate the trace buffer (stored in CBOR format) into a vector of entries.
    #[expect(clippy::expect_used)]
    pub fn hydrate(&self) -> Vec<(Instant, TraceEntry)> {
        self.messages
            .iter()
            .map(|m| from_slice(m).expect("trace buffer is not supposed to contain invalid CBOR"))
            .collect()
    }

    /// Hydrate the trace buffer (stored in CBOR format) into a vector of entries.
    #[expect(clippy::expect_used)]
    pub fn hydrate_without_timestamps(&self) -> Vec<TraceEntry> {
        self.messages
            .iter()
            .map(|m| {
                from_slice::<(Instant, TraceEntry)>(m)
                    .expect("trace buffer is not supposed to contain invalid CBOR")
                    .1
            })
            .collect()
    }

    /// Clear the trace buffer.
    pub fn clear(&mut self) {
        self.messages.clear();
        self.used_size = 0;
        self.dropped_messages = 0;
    }

    /// Get the number of entries in the trace buffer.
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// Check if the trace buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Get the total size of the entries in the trace buffer.
    pub fn used_size(&self) -> usize {
        self.used_size
    }

    /// Get the number of messages that have been dropped from the trace buffer.
    pub fn dropped_messages(&self) -> usize {
        self.dropped_messages
    }

    pub fn set_fetch_replay(&mut self, replay: std::vec::IntoIter<TraceEntry>) {
        self.fetch_replay = Some(replay);
    }

    pub fn take_fetch_replay(&mut self) -> Option<std::vec::IntoIter<TraceEntry>> {
        self.fetch_replay.take()
    }

    pub fn fetch_replay_mut(&mut self) -> Option<&mut std::vec::IntoIter<TraceEntry>> {
        self.fetch_replay.as_mut()
    }

    pub fn drop_guard(this: &Arc<Mutex<Self>>) -> DropGuard {
        DropGuard {
            buffer: this.clone(),
            active: true,
        }
    }
}

pub struct DropGuard {
    buffer: Arc<Mutex<TraceBuffer>>,
    active: bool,
}

impl DropGuard {
    pub fn defuse(mut self) {
        self.active = false;
    }
}

impl Drop for DropGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        eprintln!("Dropping trace buffer");
        for entry in self.buffer.lock().iter() {
            match from_slice::<(Instant, TraceEntry)>(entry) {
                Ok((instant, entry)) => eprintln!("{instant} {entry:?}"),
                Err(error) => eprintln!("error deserializing trace entry: {error:?}"),
            };
        }
        eprintln!("Dropped trace buffer");
    }
}

#[allow(clippy::wildcard_enum_match_arm, clippy::panic)]
pub fn find_next_external_suspend(
    fetch_replay: &mut std::vec::IntoIter<TraceEntry>,
    at_stage: &Name,
) -> Option<Box<dyn ExternalEffect>> {
    find_next(
        fetch_replay,
        |entry| entry.at_stage() == Some(at_stage),
        |entry| match entry {
            TraceEntry::Suspend(Effect::External { effect, .. }) => effect,
            entry => panic!(
                "unexpected trace entry when finding next external suspend: {:?}",
                entry
            ),
        },
    )
}

#[allow(clippy::wildcard_enum_match_arm, clippy::panic)]
pub fn find_next_external_resume(
    fetch_replay: &mut std::vec::IntoIter<TraceEntry>,
    at_stage: &Name,
) -> Option<Box<dyn SendData>> {
    find_next(
        fetch_replay,
        |entry| entry.at_stage() == Some(at_stage),
        |entry| match entry {
            TraceEntry::Resume {
                response: StageResponse::ExternalResponse(response),
                ..
            } => response,
            entry => panic!(
                "unexpected trace entry when finding next external resume: {:?}",
                entry
            ),
        },
    )
}

fn find_next<T>(
    fetch_replay: &mut std::vec::IntoIter<TraceEntry>,
    predicate: impl Fn(&TraceEntry) -> bool,
    extract: impl FnOnce(TraceEntry) -> T,
) -> Option<T> {
    let log = fetch_replay.as_mut_slice();
    let idx = log.iter().position(predicate)?;
    log[..=idx].rotate_right(1);
    fetch_replay.next().map(extract)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OutputEffect;
    use tokio::sync::mpsc;

    #[test]
    fn test_serialization() {
        let effect = OutputEffect::new(Name::from("test"), 42u32, mpsc::channel(1).0);
        let trr = TraceEntryRefRef::Suspend(EffectRef::External {
            at_stage: &Name::from("test"),
            effect: &effect,
        });
        let rr = to_cbor(&trr);
        let json = serde_json::to_string(&trr).unwrap();
        let r = to_cbor(&TraceEntryRef::Suspend(&Effect::External {
            at_stage: Name::from("test"),
            effect: Box::new(effect),
        }));
        let effect = OutputEffect::new(Name::from("test"), 42u32, mpsc::channel(1).0);
        let t = to_cbor(&TraceEntry::suspend(Effect::External {
            at_stage: Name::from("test"),
            effect: Box::new(effect),
        }));
        assert_eq!(rr, r);
        assert_eq!(rr, t);

        assert_eq!(
            json,
            r#"{"Suspend":{"External":{"at_stage":"test","effect":{"typetag":"pure_stage::output::OutputEffect<u32>","value":{"name":"test","msg":42,"sender":{}}}}}}"#
        );
    }
}
