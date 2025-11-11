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

use crate::types::as_send_data_value;
use crate::{Effect, Instant, Name, SendData, effect::StageResponse, serde::to_cbor};
use cbor4ii::serde::from_slice;
use parking_lot::Mutex;
use std::fmt::{Debug, Display, Error, Formatter};
use std::{collections::VecDeque, sync::Arc};

/// A buffer for recording the trace of a simulation.
///
/// The buffer has a bounded size and will drop the oldest entries when it is full.
/// Attempting to push an entry that would exceed the size will try to free up space, but it will
/// retain at least `min_entries`; if this does not suffice, the new entry will be dropped.
#[derive(Default)]
pub struct TraceBuffer {
    messages: VecDeque<Vec<u8>>,
    min_entries: usize,
    max_size: usize,
    used_size: usize,
    dropped_messages: usize,
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
                        | other @ StageResponse::CallTimeout
                        | other @ StageResponse::ExternalResponse(_) => format!(" -> {other}"),
                    },
                )
            }
            TraceEntry::Clock(instant) => write!(f, "clock {instant}"),
            TraceEntry::Input { stage, input } => {
                write!(
                    f,
                    "input {stage} -> {input}",
                    stage = stage.as_str(),
                    input = as_send_data_value(input.as_ref()).map_err(|_| Error)?
                )
            }
            TraceEntry::State { stage, state } => {
                write!(
                    f,
                    "state {stage} -> {state}",
                    stage = stage.as_str(),
                    state = as_send_data_value(state.as_ref()).map_err(|_| Error)?
                )
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
    External {
        at_stage: &'a Name,
        effect: &'a dyn crate::ExternalEffect,
    },
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
        }
    }

    /// Push an effect to the trace buffer.
    pub fn push_suspend(&mut self, effect: &Effect) {
        self.push(to_cbor(&TraceEntryRef::Suspend(effect)));
    }

    pub fn push_suspend_external(&mut self, at_stage: &Name, effect: &dyn crate::ExternalEffect) {
        self.push(to_cbor(&TraceEntryRefRef::Suspend(EffectRef::External {
            at_stage,
            effect,
        })));
    }

    /// Push a resume event to the trace buffer.
    pub fn push_resume(&mut self, stage: &Name, response: &StageResponse) {
        self.push(to_cbor(&TraceEntryRef::Resume { stage, response }));
    }

    pub fn push_resume_external(&mut self, stage: &Name, response: &dyn SendData) {
        self.push(to_cbor(&TraceEntryRefRef::Resume {
            stage,
            response: StageResponseRef::ExternalResponse(response),
        }));
    }

    /// Push a clock update to the trace buffer.
    pub fn push_clock(&mut self, instant: Instant) {
        self.push(to_cbor(&TraceEntryRef::Clock(instant)));
    }

    /// Push a receive event to the trace buffer.
    pub fn push_receive(&mut self, stage: &Name, input: &Box<dyn SendData>) {
        self.push(to_cbor(&TraceEntryRef::Input { stage, input }));
    }

    /// Push a state update to the trace buffer.
    ///
    /// This happens every time polling a stage yields a `Poll::Ready(Ok(...))`, i.e. as soon as the next
    /// stage state has been computed.
    pub fn push_state(&mut self, stage: &Name, state: &Box<dyn SendData>) {
        self.push(to_cbor(&TraceEntryRef::State { stage, state }));
    }

    fn push(&mut self, msg: Vec<u8>) {
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
    pub fn hydrate(&self) -> Vec<TraceEntry> {
        self.messages
            .iter()
            .map(|m| from_slice(m).expect("trace buffer is not supposed to contain invalid CBOR"))
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
