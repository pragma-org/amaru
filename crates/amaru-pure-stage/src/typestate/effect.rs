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
//! [`SendAny<R>`] is “any mailbox payload to role `R`.” Other variants (`Call`,
//! `Clock`, …) are reserved and not selected by [`Session`](super::Session) yet.

use std::{any::type_name, fmt, marker::PhantomData};

use crate::ExternalEffect;

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

/// Kleene star of a single effect or of a sequence (`Repeat<Cons<A, Cons<B, Nil>>>`).
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

pub struct Call<T>(PhantomData<T>);
impl<T> Effect for Call<T> {
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Call<{}>", type_name::<T>())
    }
}

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

pub struct Schedule<T>(PhantomData<T>);
impl<T> Effect for Schedule<T> {
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Schedule<{}>", type_name::<T>())
    }
}

pub struct CancelSchedule;
impl Effect for CancelSchedule {
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CancelSchedule")
    }
}

/// Replace the current protocol timeout (see [`Session::set_timeout`](super::Session::set_timeout)).
pub struct SetTimeout;
impl Effect for SetTimeout {
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SetTimeout")
    }
}

/// Cancel the current protocol timeout (see [`Session::clear_timeout`](super::Session::clear_timeout)).
pub struct ClearTimeout;
impl Effect for ClearTimeout {
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ClearTimeout")
    }
}

pub struct External<E: ExternalEffect>(PhantomData<E>);
impl<E: ExternalEffect> Effect for External<E> {
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "External<{}>", type_name::<E>())
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
