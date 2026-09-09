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

//! Const pretty-print of a remainder so rustc / rust-analyzer can put the
//! surface syntax in a type (`Remainder<"Send<Role, T> => Idle">`).

use std::{any::type_name, fmt, marker::PhantomData};

use super::{
    State,
    effect::{
        AddStage, Call, CancelSchedule, ClearTimeout, Clock, External, Receive, Repeat, Schedule, Send, SendAny,
        SetTimeout, Terminate, Wait,
    },
    list::{Choice, Par, Then, Uncons},
};

/// Const `&'static str` form of a remainder or effect (`Send<Role, T> => Idle`).
pub trait ConstDesc {
    const TEXT: &'static str;
}

/// ZST whose const parameter is the pretty remainder. Hover a binding of this
/// type (or ascribe a guess) to read the session at that point.
pub struct Remainder<const TEXT: &'static str>;

impl<const TEXT: &'static str> Remainder<TEXT> {
    /// The pretty remainder string (same as [`TEXT`](Self::TEXT) the type carries).
    pub const TEXT: &'static str = TEXT;
}

impl<const TEXT: &'static str> fmt::Debug for Remainder<TEXT> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(TEXT)
    }
}

impl<const TEXT: &'static str> fmt::Display for Remainder<TEXT> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(TEXT)
    }
}

const fn write_str(buf: &mut [u8], mut pos: usize, s: &str) -> usize {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        buf[pos] = b[i];
        pos += 1;
        i += 1;
    }
    pos
}

const fn join_len(a: &str, sep: &str, b: &str) -> usize {
    a.len() + sep.len() + b.len()
}

const fn wrap_len(pre: &str, inner: &str, suf: &str) -> usize {
    pre.len() + inner.len() + suf.len()
}

const fn angle2_len<R, T>(name: &str) -> usize {
    name.len() + 1 + type_name::<R>().len() + 2 + type_name::<T>().len() + 1
}

const fn angle1_len<R>(name: &str) -> usize {
    name.len() + 1 + type_name::<R>().len() + 1
}

const fn str_from_bytes(bytes: &[u8]) -> &str {
    // Only `write_str` of UTF-8 literals and `type_name` output fills these buffers.
    unsafe { core::str::from_utf8_unchecked(bytes) }
}

struct Angle2<R, T, const NAME: &'static str, const N: usize>(PhantomData<(R, T)>);
impl<R, T, const NAME: &'static str, const N: usize> Angle2<R, T, NAME, N> {
    const BYTES: [u8; N] = {
        let mut buf = [0u8; N];
        let mut pos = 0;
        pos = write_str(&mut buf, pos, NAME);
        pos = write_str(&mut buf, pos, "<");
        pos = write_str(&mut buf, pos, type_name::<R>());
        pos = write_str(&mut buf, pos, ", ");
        pos = write_str(&mut buf, pos, type_name::<T>());
        let _ = write_str(&mut buf, pos, ">");
        buf
    };
    const TEXT: &'static str = str_from_bytes(&Self::BYTES);
}

struct Angle1<R, const NAME: &'static str, const N: usize>(PhantomData<R>);
impl<R, const NAME: &'static str, const N: usize> Angle1<R, NAME, N> {
    const BYTES: [u8; N] = {
        let mut buf = [0u8; N];
        let mut pos = 0;
        pos = write_str(&mut buf, pos, NAME);
        pos = write_str(&mut buf, pos, "<");
        pos = write_str(&mut buf, pos, type_name::<R>());
        let _ = write_str(&mut buf, pos, ">");
        buf
    };
    const TEXT: &'static str = str_from_bytes(&Self::BYTES);
}

struct JoinBuf<A, B, const SEP: &'static str, const N: usize>(PhantomData<(A, B)>);
impl<A: ConstDesc, B: ConstDesc, const SEP: &'static str, const N: usize> JoinBuf<A, B, SEP, N> {
    const BYTES: [u8; N] = {
        let mut buf = [0u8; N];
        let mut pos = 0;
        pos = write_str(&mut buf, pos, A::TEXT);
        pos = write_str(&mut buf, pos, SEP);
        let _ = write_str(&mut buf, pos, B::TEXT);
        buf
    };
    const TEXT: &'static str = str_from_bytes(&Self::BYTES);
}

struct WrapBuf<T, const PRE: &'static str, const SUF: &'static str, const N: usize>(PhantomData<T>);
impl<T: ConstDesc, const PRE: &'static str, const SUF: &'static str, const N: usize> WrapBuf<T, PRE, SUF, N> {
    const BYTES: [u8; N] = {
        let mut buf = [0u8; N];
        let mut pos = 0;
        pos = write_str(&mut buf, pos, PRE);
        pos = write_str(&mut buf, pos, T::TEXT);
        let _ = write_str(&mut buf, pos, SUF);
        buf
    };
    const TEXT: &'static str = str_from_bytes(&Self::BYTES);
}

struct ThenBuf<P, S, const N: usize>(PhantomData<(P, S)>);
impl<P: ConstDesc, S: State, const N: usize> ThenBuf<P, S, N> {
    const BYTES: [u8; N] = {
        let mut buf = [0u8; N];
        let mut pos = 0;
        pos = write_str(&mut buf, pos, P::TEXT);
        pos = write_str(&mut buf, pos, " => ");
        let _ = write_str(&mut buf, pos, S::NAME);
        buf
    };
    const TEXT: &'static str = str_from_bytes(&Self::BYTES);
}

struct EmptyThenBuf<S: State, const N: usize>(PhantomData<S>);
impl<S: State, const N: usize> EmptyThenBuf<S, N> {
    const BYTES: [u8; N] = {
        let mut buf = [0u8; N];
        let mut pos = 0;
        pos = write_str(&mut buf, pos, "=> ");
        let _ = write_str(&mut buf, pos, S::NAME);
        buf
    };
    const TEXT: &'static str = str_from_bytes(&Self::BYTES);
}

macro_rules! const_lit {
    ($ty:ty, $text:expr) => {
        impl ConstDesc for $ty {
            const TEXT: &'static str = $text;
        }
    };
}

const_lit!(Wait, "Wait");
const_lit!(Clock, "Clock");
const_lit!(Terminate, "Terminate");
const_lit!(CancelSchedule, "CancelSchedule");
const_lit!(SetTimeout, "SetTimeout");
const_lit!(ClearTimeout, "ClearTimeout");
const_lit!(AddStage, "AddStage");

impl<R, T> ConstDesc for Send<R, T>
where
    [(); angle2_len::<R, T>("Send")]:,
{
    const TEXT: &'static str = Angle2::<R, T, "Send", { angle2_len::<R, T>("Send") }>::TEXT;
}

impl<R, T> ConstDesc for Call<R, T>
where
    [(); angle2_len::<R, T>("Call")]:,
{
    const TEXT: &'static str = Angle2::<R, T, "Call", { angle2_len::<R, T>("Call") }>::TEXT;
}

impl<R> ConstDesc for SendAny<R>
where
    [(); angle1_len::<R>("SendAny")]:,
{
    const TEXT: &'static str = Angle1::<R, "SendAny", { angle1_len::<R>("SendAny") }>::TEXT;
}

impl<T> ConstDesc for Receive<T>
where
    [(); angle1_len::<T>("Receive")]:,
{
    const TEXT: &'static str = Angle1::<T, "Receive", { angle1_len::<T>("Receive") }>::TEXT;
}

impl<T> ConstDesc for Schedule<T>
where
    [(); angle1_len::<T>("Schedule")]:,
{
    const TEXT: &'static str = Angle1::<T, "Schedule", { angle1_len::<T>("Schedule") }>::TEXT;
}

impl<E: crate::ExternalEffect> ConstDesc for External<E>
where
    [(); angle1_len::<E>("External")]:,
{
    const TEXT: &'static str = Angle1::<E, "External", { angle1_len::<E>("External") }>::TEXT;
}

impl<E: ConstDesc> ConstDesc for Repeat<E>
where
    [(); wrap_len("Repeat<", E::TEXT, ">")]:,
{
    const TEXT: &'static str = WrapBuf::<E, "Repeat<", ">", { wrap_len("Repeat<", E::TEXT, ">") }>::TEXT;
}

macro_rules! impl_const_seq {
    ($H:ident) => {
        impl<$H: ConstDesc> ConstDesc for ($H,) {
            const TEXT: &'static str = $H::TEXT;
        }
    };
    ($H:ident, $($T:ident),+) => {
        impl<$H: ConstDesc, $($T: ConstDesc),+> ConstDesc for ($H, $($T,)+)
        where
            ($($T,)+): ConstDesc,
            [(); $H::TEXT.len() $(+ 2 + $T::TEXT.len())*]:,
        {
            const TEXT: &'static str = JoinBuf::<
                $H,
                ($($T,)+),
                ", ",
                { $H::TEXT.len() $(+ 2 + $T::TEXT.len())* },
            >::TEXT;
        }
        impl_const_seq!($($T),+);
    };
}

macro_rules! impl_const_par {
    ($H:ident) => {
        impl<$H: ConstDesc> ConstDesc for Par<($H,)> {
            const TEXT: &'static str = $H::TEXT;
        }
        impl<$H: ConstDesc> ConstDesc for Choice<($H,)> {
            const TEXT: &'static str = $H::TEXT;
        }
    };
    ($H:ident, $($T:ident),+) => {
        impl<$H: ConstDesc, $($T: ConstDesc),+> ConstDesc for Par<($H, $($T,)+)>
        where
            Par<($($T,)+)>: ConstDesc,
            [(); $H::TEXT.len() $(+ 3 + $T::TEXT.len())*]:,
        {
            const TEXT: &'static str = JoinBuf::<
                $H,
                Par<($($T,)+)>,
                " | ",
                { $H::TEXT.len() $(+ 3 + $T::TEXT.len())* },
            >::TEXT;
        }
        impl<$H: ConstDesc, $($T: ConstDesc),+> ConstDesc for Choice<($H, $($T,)+)>
        where
            Choice<($($T,)+)>: ConstDesc,
            [(); $H::TEXT.len() $(+ 3 + $T::TEXT.len())*]:,
        {
            const TEXT: &'static str = JoinBuf::<
                $H,
                Choice<($($T,)+)>,
                " | ",
                { $H::TEXT.len() $(+ 3 + $T::TEXT.len())* },
            >::TEXT;
        }
        impl_const_par!($($T),+);
    };
}

impl_const_seq!(E0, E1, E2, E3, E4, E5, E6, E7, E8, E9);
impl_const_par!(B0, B1, B2, B3, B4, B5, B6, B7, B8, B9);

impl ConstDesc for Par<()> {
    const TEXT: &'static str = "(none)";
}

impl<S: State> ConstDesc for Then<Par<()>, S>
where
    [(); wrap_len("=> ", S::NAME, "")]:,
{
    const TEXT: &'static str = EmptyThenBuf::<S, { wrap_len("=> ", S::NAME, "") }>::TEXT;
}

impl<P, S: State> ConstDesc for Then<Par<P>, S>
where
    P: Uncons,
    Par<P>: ConstDesc,
    [(); join_len(<Par<P> as ConstDesc>::TEXT, " => ", S::NAME)]:,
{
    const TEXT: &'static str = ThenBuf::<Par<P>, S, { join_len(<Par<P> as ConstDesc>::TEXT, " => ", S::NAME) }>::TEXT;
}
