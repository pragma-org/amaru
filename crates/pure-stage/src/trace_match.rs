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

//! Utilities for flexible matching against [`TraceEntry`] values recorded in a
//! [`SimulationRunning`] trace buffer. This module provides [`TraceMatch`], a
//! type that can represent either an exact [`TraceEntry`] or a property-based
//! predicate, along with ergonomic assertion helpers and `tm_*` constructors
//! (the matching counterparts to the `te_*` effect constructors).

use std::fmt;

use crate::{Effect, Name, SendData, serde::SendDataValue, simulation::SimulationRunning, trace_buffer::TraceEntry};

/// A matcher for a [`TraceEntry`].
///
/// `TraceMatch` can be used in assertions where you either want an exact match
/// (via [`TraceMatch::Literal`], which `TraceEntry` converts into via `From`)
/// or a flexible property match (via [`TraceMatch::Property`]).
///
/// This is particularly useful when the exact value of a field (such as a
/// dynamically generated stage name from `Effects::stage`) is not known ahead
/// of time, but a predicate on it (e.g. "the name starts with 'foo'") can be
/// checked.
pub enum TraceMatch<'a> {
    /// An exact [`TraceEntry`] that must match.
    Literal(TraceEntry),
    /// A predicate on a [`TraceEntry`] together with a human-readable
    /// description used for `Debug` output on assertion failure.
    Property(Box<dyn Fn(&TraceEntry) -> bool + Send + 'a>, String),
}

impl From<TraceEntry> for TraceMatch<'static> {
    fn from(entry: TraceEntry) -> Self {
        TraceMatch::Literal(entry)
    }
}

impl<'a> PartialEq<TraceEntry> for TraceMatch<'a> {
    fn eq(&self, other: &TraceEntry) -> bool {
        match self {
            TraceMatch::Literal(literal) => literal == other,
            TraceMatch::Property(predicate, _) => predicate(other),
        }
    }
}

impl<'a> PartialEq<TraceMatch<'a>> for TraceEntry {
    fn eq(&self, other: &TraceMatch<'a>) -> bool {
        match other {
            TraceMatch::Literal(literal) => self == literal,
            TraceMatch::Property(predicate, _) => predicate(self),
        }
    }
}

impl<'a> fmt::Debug for TraceMatch<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TraceMatch::Literal(literal) => fmt::Debug::fmt(literal, f),
            TraceMatch::Property(_predicate, description) => f.write_str(description),
        }
    }
}

// =============================================================================
// tm_* constructors (matching counterparts to te_*)
// =============================================================================

/// Creates a `TraceMatch` for a state entry.
pub fn tm_state<T: SendData + Clone>(stage: impl AsRef<str>, state: &T) -> TraceMatch<'static> {
    TraceEntry::State { stage: Name::from(stage.as_ref()), state: Box::new(state.clone()) }.into()
}

/// Creates a `TraceMatch` for an input entry.
pub fn tm_input<T: SendData + Clone>(stage: impl AsRef<str>, input: &T) -> TraceMatch<'static> {
    TraceEntry::Input { stage: Name::from(stage.as_ref()), input: Box::new(input.clone()) }.into()
}

/// Creates a `TraceMatch` for a `Send` effect.
pub fn tm_send<'a>(from: &'a str, to: &'a str, msg: impl SendData) -> TraceMatch<'a> {
    let description = format!("Send(from: {:?}, to: {:?}, msg: {:?})", from, to, &msg);
    TraceMatch::Property(
        Box::new(move |entry| {
            if let TraceEntry::Suspend(Effect::Send { from: f, to: t, msg: m }) = entry {
                f.as_str() == from && t.as_str().contains(to) && msg.test_eq(&**m)
            } else {
                false
            }
        }),
        description,
    )
}

pub fn tm_send_type<'a, T: SendData>(from: &'a str, to: &'a str) -> TraceMatch<'a> {
    let description = format!("Send(from: {:?}, to: {:?}, msg of type {})", from, to, std::any::type_name::<T>());
    TraceMatch::Property(
        Box::new(move |entry| {
            if let TraceEntry::Suspend(Effect::Send { from: f, to: t, msg }) = entry {
                f.as_str() == from && t.as_str().contains(to) && msg.as_ref().type_id() == std::any::TypeId::of::<T>()
            } else {
                false
            }
        }),
        description,
    )
}

/// Creates a `TraceMatch` for a `Send` effect where the message is of type `T`
/// and satisfies the given predicate.
///
/// This is useful when you want to assert the *kind* of message (e.g. `ManagerMessage::AddPeer`)
/// without caring about the exact payload (e.g. which random peer was chosen).
pub fn tm_send_match<'a, T: SendData>(
    from: &'a str,
    to: &'a str,
    predicate: impl Fn(&T) -> bool + Send + 'a,
) -> TraceMatch<'a> {
    let description = format!("Send(from: {:?}, to: {:?}, msg matching {})", from, to, std::any::type_name::<T>());
    TraceMatch::Property(
        Box::new(move |entry| {
            if let TraceEntry::Suspend(Effect::Send { from: f, to: t, msg }) = entry {
                if f.as_str() != from || !t.as_str().contains(to) {
                    return false;
                }
                if let Ok(typed) = msg.as_ref().cast_ref::<T>() { predicate(typed) } else { false }
            } else {
                false
            }
        }),
        description,
    )
}

/// Creates a `TraceMatch` for a `Terminate` effect.
pub fn tm_terminate(at_stage: impl AsRef<str>) -> TraceMatch<'static> {
    TraceEntry::suspend(Effect::Terminate { at_stage: Name::from(at_stage.as_ref()) }).into()
}

/// Creates a `TraceMatch` for a `Terminated` trace entry.
pub fn tm_terminated(at_stage: impl AsRef<str>, reason: crate::trace_buffer::TerminationReason) -> TraceMatch<'static> {
    TraceEntry::Terminated { stage: Name::from(at_stage.as_ref()), reason }.into()
}

/// Creates a `TraceMatch` for an `AddStage` effect with an exact name.
pub fn tm_add_stage(at_stage: impl AsRef<str>, name: impl AsRef<str>) -> TraceMatch<'static> {
    TraceEntry::suspend(Effect::AddStage { at_stage: Name::from(at_stage.as_ref()), name: Name::from(name.as_ref()) })
        .into()
}

pub fn tm_wire_stage<'a>(parent: &'a str, child: &'a str) -> TraceMatch<'a> {
    let description = format!("WireStage(at_stage: {:?}, name: {:?})", parent, child);
    TraceMatch::Property(
        Box::new(move |entry| {
            if let TraceEntry::Suspend(Effect::WireStage { at_stage, name, .. }) = entry {
                parent == at_stage.as_str() && name.as_str().contains(child)
            } else {
                false
            }
        }),
        description,
    )
}

pub fn tm_wire_stage_state<'a, T: SendData>(parent: &'a str, child: &'a str, state: T) -> TraceMatch<'a> {
    let description = format!("WireStage(at_stage: {:?}, name: {:?}, state: {:?})", parent, child, state);
    TraceMatch::Property(
        Box::new(move |entry| {
            if let TraceEntry::Suspend(Effect::WireStage { at_stage, name, initial_state, tombstone }) = entry {
                parent == at_stage.as_str()
                    && name.as_str().contains(child)
                    && state.test_eq(&**initial_state)
                    && tombstone
                        .cast_ref::<SendDataValue>()
                        .is_ok_and(|v| v.typetag == "pure_stage::effect::CanSupervise")
            } else {
                false
            }
        }),
        description,
    )
}

pub fn tm_wire_stage_state_supervised<'a, T: SendData, U: SendData>(
    parent: &'a str,
    child: &'a str,
    state: T,
    supervision: U,
) -> TraceMatch<'a> {
    let description = format!(
        "WireStage(at_stage: {:?}, name: {:?}, state: {:?}, tombstone: {:?})",
        parent, child, state, supervision
    );
    TraceMatch::Property(
        Box::new(move |entry| {
            if let TraceEntry::Suspend(Effect::WireStage { at_stage, name, initial_state, tombstone }) = entry {
                parent == at_stage.as_str()
                    && name.as_str().contains(child)
                    && state.test_eq(&**initial_state)
                    && supervision.test_eq(tombstone)
            } else {
                false
            }
        }),
        description,
    )
}

// =============================================================================
// Assertion helpers
// =============================================================================

fn collect_filtered_trace(running: &SimulationRunning) -> Vec<TraceEntry> {
    let mut tb = running.trace_buffer().lock();
    let trace: Vec<_> =
        tb.iter_entries().filter_map(|(_, e)| (!matches!(e, TraceEntry::Resume { .. })).then_some(e)).collect();
    tb.clear();
    trace
}

/// Asserts that the filtered trace (excluding `Resume` entries) exactly equals
/// the provided sequence of [`TraceMatch`] values.
///
/// Each element of `expected` may be either a literal [`TraceEntry`] (via the
/// `From` impl) or a property matcher.
#[track_caller]
pub fn assert_trace_match(running: &SimulationRunning, expected: &[TraceMatch<'_>]) {
    let trace = collect_filtered_trace(running);
    pretty_assertions::assert_eq!(trace, expected);
}

/// Asserts that the filtered trace contains the given sequence of
/// [`TraceMatch`] values **in order**, but not necessarily consecutively
/// (i.e. it is a subsequence match).
///
/// Non-matching entries in the actual trace are skipped when looking for the
/// next expected matcher.
#[track_caller]
#[expect(clippy::panic)]
pub fn assert_trace_contains(running: &SimulationRunning, expected: &[TraceMatch<'_>]) {
    let trace = collect_filtered_trace(running);
    let mut i = 0usize;

    for entry in &trace {
        if i < expected.len() && expected[i] == *entry {
            i += 1;
        }
    }

    if i < expected.len() {
        let expected = expected
            .iter()
            .enumerate()
            .map(|(line, m)| format!("{} {:?}\n", if line >= i { "!" } else { " " }, m))
            .collect::<String>();
        let trace = trace.iter().map(|e| format!("{:?}\n", e)).collect::<String>();
        panic!(
            "expected trace to contain the following sequence as a subsequence:\n\n{expected}\nactual trace:\n{trace}"
        );
    }
}

/// Asserts that none of the provided [`TraceMatch`] values appear anywhere
/// in the filtered trace.
#[track_caller]
#[expect(clippy::panic)]
pub fn assert_trace_does_not_contain(running: &SimulationRunning, forbidden: &[TraceMatch<'_>]) {
    let trace = collect_filtered_trace(running);

    for entry in &trace {
        for f in forbidden {
            if f == entry {
                panic!(
                    "trace contained a forbidden entry:\nentry = {entry:?}\nforbidden pattern = {f:?}\n\nfull trace:\n{trace:?}"
                );
            }
        }
    }
}
