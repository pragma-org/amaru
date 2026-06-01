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

use std::{any::type_name, fmt, marker::PhantomData};

use crate::{ExternalEffect, SendData};

pub trait Effect {
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result;
}

pub struct Send<T: SendData>(PhantomData<T>);
impl<T: SendData> Effect for Send<T> {
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Send<{}>", type_name::<T>())
    }
}
pub struct Receive<T: SendData>(PhantomData<T>);
impl<T: SendData> Effect for Receive<T> {
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Receive<{}>", type_name::<T>())
    }
}
pub struct Call<T: SendData>(PhantomData<T>);
impl<T: SendData> Effect for Call<T> {
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Call<{}>", type_name::<T>())
    }
}
pub struct Clock();
impl Effect for Clock {
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Clock")
    }
}
pub struct Wait();
impl Effect for Wait {
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Wait")
    }
}
pub struct Schedule<T: SendData>(PhantomData<T>);
impl<T: SendData> Effect for Schedule<T> {
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Schedule<{}>", type_name::<T>())
    }
}
pub struct CancelSchedule();
impl Effect for CancelSchedule {
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CancelSchedule")
    }
}
pub struct External<E: ExternalEffect>(PhantomData<E>);
impl<E: ExternalEffect> Effect for External<E> {
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "External<{}>", type_name::<E>())
    }
}
pub struct Terminate();
impl Effect for Terminate {
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Terminate")
    }
}
pub struct AddStage();
impl Effect for AddStage {
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AddStage")
    }
}

pub struct Repeat<E: Effect, const MIN: usize, const MAX: usize>(PhantomData<E>);
impl<E: Effect, const MIN: usize, const MAX: usize> Effect for Repeat<E, MIN, MAX> {
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Repeat<")?;
        E::fmt(f)?;
        write!(f, ", {}, {}>", MIN, MAX)
    }
}
