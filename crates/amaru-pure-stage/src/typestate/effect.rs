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

//! Phantom tags that appear in a remainder list. No runtime data; [`Effect::fmt`]
//! is for diagnostics. [`Repeat<E>`] is Kleene star (use does not consume it).
//! [`SendAny<R>`] is “any mailbox payload to role `R`.” [`Call<R, T>`] is a
//! request/response to role `R`. [`Clock`], [`External`], [`Detach`],
//! [`Schedule`], and [`CancelSchedule`] are selected by [`Session`](super::Session)
//! the same way as [`Wait`] / [`SetTimeout`].

use std::{any::type_name, fmt, marker::PhantomData};

use crate::ExternalEffect;

/// Last outermost path segment of `type_name::<T>()` (generic args preserved).
///
/// Path types lose their module prefix (`foo::Bar` → `Bar`,
/// `foo::Bar<baz::Qux>` → `Bar<baz::Qux>`). Tuples, arrays, slices, references,
/// pointers, and function types are returned whole: a `::` inside them is not a
/// prefix of the outer type, and stripping it can invent a payload name
/// (`fn(...) -> baz::Qux` → `Qux`).
pub(super) const fn type_last_segment<T>() -> &'static str {
    last_segment(type_name::<T>())
}

/// Last `::` at angle-bracket depth 0 of a path type.
///
/// `Option<foo::Bar>` is `"Option<foo::Bar>"`, not `"Bar>"`. A type that is not
/// a path is returned unchanged.
const fn last_segment(name: &'static str) -> &'static str {
    let bytes = name.as_bytes();
    if !is_path_type(bytes) {
        return name;
    }
    let mut i = bytes.len();
    let mut depth = 0usize;
    while i > 0 {
        i -= 1;
        match bytes[i] {
            b'>' => depth += 1,
            b'<' => depth = depth.saturating_sub(1),
            b':' if depth == 0 => return name.split_at(i + 1).1,
            _ => {}
        }
    }
    name
}

const fn is_ident_start(b: u8) -> bool {
    matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'_')
}

const fn is_ident_continue(b: u8) -> bool {
    matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_')
}

/// Index just past the identifier starting at `at`, if there is one.
const fn ident_end(bytes: &[u8], at: usize) -> Option<usize> {
    if at >= bytes.len() || !is_ident_start(bytes[at]) {
        return None;
    }
    let mut i = at + 1;
    while i < bytes.len() && is_ident_continue(bytes[i]) {
        i += 1;
    }
    Some(i)
}

/// `foo::Bar` or `foo::Bar<...>`, and nothing outside the angle brackets.
const fn is_path_type(bytes: &[u8]) -> bool {
    let Some(mut i) = ident_end(bytes, 0) else {
        return false;
    };
    while i + 1 < bytes.len() && bytes[i] == b':' && bytes[i + 1] == b':' {
        let Some(next) = ident_end(bytes, i + 2) else {
            return false;
        };
        i = next;
    }
    if i == bytes.len() {
        return true;
    }
    if bytes[i] != b'<' {
        return false;
    }
    let mut depth = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'<' => depth += 1,
            b'>' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return i + 1 == bytes.len();
                }
            }
            _ => {}
        }
        i += 1;
    }
    false
}

/// A type-level tag for an effect that can appear in a session remainder.
pub trait Effect {
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result;
}

/// Send payload `T` to role `R`.
///
/// `R` wraps a [`StageRef`](crate::StageRef) whose mailbox implements [`From<T>`].
pub struct Send<R, T>(PhantomData<(R, T)>);
impl<R, T> Effect for Send<R, T> {
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Send<{}, {}>", type_name::<R>(), type_name::<T>())
    }
}

/// Send any mailbox-typed message to role `R`.
pub struct SendAny<R>(PhantomData<R>);
impl<R> Effect for SendAny<R> {
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SendAny<{}>", type_name::<R>())
    }
}

/// Kleene star of a single effect or of a sequence (`Repeat<(A, B)>`).
///
/// Selecting the first step unrolls the rest in front of the same `Repeat`.
/// Selecting the following step discards the star (zero iterations).
pub struct Repeat<E>(PhantomData<E>);
impl<E: Effect> Effect for Repeat<E> {
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Repeat<")?;
        E::fmt(f)?;
        write!(f, ">")
    }
}

/// Receive a value of type `T`. Never appears in a [`Session`](super::Session)
/// remainder: it is consumed by [`State::receive`](super::State::receive).
pub struct Receive<T>(PhantomData<T>);
impl<T> Effect for Receive<T> {
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Receive<{}>", type_name::<T>())
    }
}

/// Call role `R` with payload `T` and wait for the reply (or timeout).
///
/// `R` wraps a [`StageRef`](crate::StageRef). The reply type is
/// [`IntoRoleCall::Reply`](super::IntoRoleCall).
pub struct Call<R, T>(PhantomData<(R, T)>);
impl<R, T> Effect for Call<R, T> {
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Call<{}, {}>", type_name::<R>(), type_name::<T>())
    }
}

/// Read the current time (see [`Session::clock`](super::Session::clock)).
pub struct Clock;
impl Effect for Clock {
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Clock")
    }
}

pub struct Wait;
impl Effect for Wait {
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Wait")
    }
}

/// Schedule a mailbox message at a future instant (see [`Session::schedule_at`](super::Session::schedule_at)).
pub struct Schedule<T>(PhantomData<T>);
impl<T> Effect for Schedule<T> {
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Schedule<{}>", type_name::<T>())
    }
}

/// Cancel a previously scheduled message (see [`Session::cancel_schedule`](super::Session::cancel_schedule)).
pub struct CancelSchedule;
impl Effect for CancelSchedule {
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CancelSchedule")
    }
}

/// Replace the current protocol timeout (see [`SessionOps::set_timeout`](super::SessionOps::set_timeout)).
pub struct SetTimeout;
impl Effect for SetTimeout {
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SetTimeout")
    }
}

/// Cancel the current protocol timeout (see [`SessionOps::clear_timeout`](super::SessionOps::clear_timeout)).
pub struct ClearTimeout;
impl Effect for ClearTimeout {
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ClearTimeout")
    }
}

/// Run an external effect and wait for its response (see [`Session::external`](super::Session::external)).
pub struct External<E: ExternalEffect>(PhantomData<E>);
impl<E: ExternalEffect> Effect for External<E> {
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "External<{}>", type_name::<E>())
    }
}

/// Start an external effect without occupying the airlock (see [`Session::detach`](super::Session::detach)).
pub struct Detach<E: ExternalEffect>(PhantomData<E>);
impl<E: ExternalEffect> Effect for Detach<E> {
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Detach<{}>", type_name::<E>())
    }
}

pub struct Terminate;
impl Effect for Terminate {
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Terminate")
    }
}

pub struct AddStage;
impl Effect for AddStage {
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AddStage")
    }
}

#[cfg(test)]
mod tests {
    use super::last_segment;

    #[test]
    fn last_segment_skips_colons_inside_generic_args() {
        assert_eq!(last_segment("core::option::Option<foo::Bar>"), "Option<foo::Bar>");
        assert_ne!(last_segment("core::option::Option<foo::Bar>"), "Bar>");
    }

    #[test]
    fn last_segment_plain_path_and_nested_generics() {
        assert_eq!(last_segment("u8"), "u8");
        assert_eq!(last_segment("foo::Bar"), "Bar");
        assert_eq!(last_segment("foo::Bar<baz::Qux>"), "Bar<baz::Qux>");
        assert_eq!(last_segment("core::result::Result<foo::Bar, baz::Qux>"), "Result<foo::Bar, baz::Qux>");
        assert_eq!(
            last_segment("core::option::Option<alloc::boxed::Box<foo::Bar>>"),
            "Option<alloc::boxed::Box<foo::Bar>>"
        );
    }

    #[test]
    fn last_segment_keeps_composite_types_whole() {
        assert_eq!(last_segment("(foo::Bar, u8)"), "(foo::Bar, u8)");
        assert_eq!(last_segment("[foo::Bar; 4]"), "[foo::Bar; 4]");
        assert_eq!(last_segment("fn(foo::Bar) -> baz::Qux"), "fn(foo::Bar) -> baz::Qux");
        assert_eq!(last_segment("&foo::Bar"), "&foo::Bar");
        assert_eq!(last_segment("&mut foo::Bar"), "&mut foo::Bar");
        assert_eq!(last_segment("*const foo::Bar"), "*const foo::Bar");
        assert_eq!(last_segment("*mut foo::Bar"), "*mut foo::Bar");
    }

    mod foo {
        pub struct Bar;
    }
    mod baz {
        pub struct Qux;
    }

    #[test]
    fn type_last_segment_of_real_composites_does_not_invent_a_label() {
        let tuple = super::type_last_segment::<(foo::Bar, u8)>();
        assert!(tuple.starts_with('('), "{tuple}");
        assert!(tuple.contains("foo::Bar"), "{tuple}");
        assert_ne!(tuple, "Bar, u8)");

        let array = super::type_last_segment::<[foo::Bar; 4]>();
        assert!(array.starts_with('['), "{array}");
        assert!(array.contains("foo::Bar"), "{array}");
        assert_ne!(array, "Bar; 4]");

        let function = super::type_last_segment::<fn(foo::Bar) -> baz::Qux>();
        assert!(function.starts_with("fn("), "{function}");
        assert!(function.contains("baz::Qux"), "{function}");
        assert_ne!(function, "Qux");

        assert_eq!(super::type_last_segment::<foo::Bar>(), "Bar");
        assert!(super::type_last_segment::<&foo::Bar>().starts_with('&'));
    }
}
