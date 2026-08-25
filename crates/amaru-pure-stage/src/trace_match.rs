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

use std::{fmt, time::Duration};

use crate::{
    EPOCH, Effect, ExternalEffect, Name, SendData, serde::SendDataValue, simulation::SimulationRunning,
    trace_buffer::TraceEntry,
};

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

/// Creates a `TraceMatch` for a state entry.
pub fn tm_state_match<'a, T: SendData>(stage: &'a str, predicate: impl Fn(&T) -> bool + Send + 'a) -> TraceMatch<'a> {
    let description = format!("State(stage: {:?}, state matching {})", stage, std::any::type_name::<T>());
    TraceMatch::Property(
        Box::new(move |entry| {
            let TraceEntry::State { stage: s, state } = entry else {
                return false;
            };
            if s.as_str() != stage {
                return false;
            }
            let Ok(typed) = state.as_ref().cast_ref::<T>() else {
                return false;
            };
            predicate(typed)
        }),
        description,
    )
}

/// Creates a `TraceMatch` for an input entry.
pub fn tm_input<T: SendData + Clone>(stage: impl AsRef<str>, input: &T) -> TraceMatch<'static> {
    TraceEntry::Input { stage: Name::from(stage.as_ref()), input: Box::new(input.clone()) }.into()
}

/// Creates a `TraceMatch` for a `Send` effect.
pub fn tm_send<'a>(from: &'a str, to: &'a str, msg: impl SendData) -> TraceMatch<'a> {
    let description = format!("Send(from: {:?}, to: {:?}, msg: {:?})", from, to, msg);
    TraceMatch::Property(
        Box::new(move |entry| {
            let TraceEntry::Suspend(Effect::Send { from: f, to: t, msg: m }) = entry else {
                return false;
            };
            f.as_str() == from && t.as_str().contains(to) && msg.test_eq(&**m)
        }),
        description,
    )
}

pub fn tm_send_type<'a, T: SendData>(from: &'a str, to: &'a str) -> TraceMatch<'a> {
    let description = format!("Send(from: {:?}, to: {:?}, msg of type {})", from, to, std::any::type_name::<T>());
    TraceMatch::Property(
        Box::new(move |entry| {
            let TraceEntry::Suspend(Effect::Send { from: f, to: t, msg }) = entry else {
                return false;
            };
            f.as_str() == from && t.as_str().contains(to) && msg.as_ref().type_id() == std::any::TypeId::of::<T>()
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
            let TraceEntry::Suspend(Effect::Send { from: f, to: t, msg }) = entry else {
                return false;
            };
            if f.as_str() != from || !t.as_str().contains(to) {
                return false;
            }
            let Ok(typed) = msg.as_ref().cast_ref::<T>() else {
                return false;
            };
            predicate(typed)
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
            let TraceEntry::Suspend(Effect::WireStage { at_stage, name, .. }) = entry else {
                return false;
            };
            parent == at_stage.as_str() && name.as_str().contains(child)
        }),
        description,
    )
}

pub fn tm_wire_stage_state<'a, T: SendData>(parent: &'a str, child: &'a str, state: T) -> TraceMatch<'a> {
    let description = format!("WireStage(at_stage: {:?}, name: {:?}, state: {:?})", parent, child, state);
    TraceMatch::Property(
        Box::new(move |entry| {
            let TraceEntry::Suspend(Effect::WireStage { at_stage, name, initial_state, tombstone }) = entry else {
                return false;
            };
            parent == at_stage.as_str()
                && name.as_str().contains(child)
                && state.test_eq(&**initial_state)
                && tombstone
                    .cast_ref::<SendDataValue>()
                    .is_ok_and(|v| v.typetag == "amaru_pure_stage::effect::CanSupervise")
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
            let TraceEntry::Suspend(Effect::WireStage { at_stage, name, initial_state, tombstone }) = entry else {
                return false;
            };
            parent == at_stage.as_str()
                && name.as_str().contains(child)
                && state.test_eq(&**initial_state)
                && supervision.test_eq(tombstone)
        }),
        description,
    )
}

/// Creates a `TraceMatch` that matches any `Suspend(External { .., effect })`
/// whose effect downcasts to the given `T`. Matches both blocking
/// [`crate::Effect::External`] and [`crate::Effect::Detach`].
///
/// This is the generic form for "I expect this external effect to have been
/// performed, but I don't care about (or can't easily name) its exact payload".
pub fn tm_external_effect<T: ExternalEffect>(at_stage: impl AsRef<str>) -> TraceMatch<'static> {
    tm_external_effect_match::<T>(at_stage, |_| true, Detached::Either)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detached {
    Yes,
    No,
    Either,
}

impl Detached {
    fn allows_inline(&self) -> bool {
        matches!(self, Detached::No | Detached::Either)
    }
    fn allows_detached(&self) -> bool {
        matches!(self, Detached::Yes | Detached::Either)
    }
}

/// Creates a `TraceMatch` that matches `Suspend(External { at_stage, effect })`
/// or `Suspend(Detach { at_stage, effect })` where the effect casts to `T` **and**
/// the provided predicate holds on it.
///
/// Use the simple `tm_external_effect::<T>(at_stage)` when only presence/type matters.
pub fn tm_external_effect_match<'a, T: ExternalEffect>(
    at_stage: impl AsRef<str>,
    predicate: impl Fn(&T) -> bool + Send + 'a,
    detached: Detached,
) -> TraceMatch<'a> {
    let stage_name = Name::from(at_stage.as_ref());
    let description = format!("ExternalEffect<{}>(at_stage: {:?})", std::any::type_name::<T>(), stage_name);
    TraceMatch::Property(
        #[expect(clippy::wildcard_enum_match_arm)]
        Box::new(move |entry| {
            let (at_stage, effect) = match entry {
                TraceEntry::Suspend(Effect::External { at_stage, effect }) if detached.allows_inline() => {
                    (at_stage, effect)
                }
                TraceEntry::Suspend(Effect::Detach { at_stage, effect }) if detached.allows_detached() => {
                    (at_stage, effect)
                }
                _ => return false,
            };
            if at_stage != &stage_name {
                return false;
            }
            let Some(typed) = effect.cast_ref::<T>() else {
                return false;
            };
            predicate(typed)
        }),
        description,
    )
}

pub fn tm_clock(instant: Duration) -> TraceMatch<'static> {
    let description = format!("Clock({:?})", instant);
    TraceMatch::Property(
        Box::new(move |entry| {
            let TraceEntry::Clock(i) = entry else {
                return false;
            };
            i.inner.saturating_duration_since(*EPOCH) == instant
        }),
        description,
    )
}

/// Matches a [`TraceEntry::Clock`] whose sim-elapsed time lies in `[min, max]` inclusive.
pub fn tm_clock_between(min: Duration, max: Duration) -> TraceMatch<'static> {
    let description = format!("Clock(between {:?} and {:?})", min, max);
    TraceMatch::Property(
        Box::new(move |entry| {
            let TraceEntry::Clock(i) = entry else {
                return false;
            };
            let elapsed = i.inner.saturating_duration_since(*EPOCH);
            elapsed >= min && elapsed <= max
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
