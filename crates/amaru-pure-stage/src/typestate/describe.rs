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

/// Always panics with [`ConstDesc::TEXT`]. Used by [`reveal_remainder`](crate::reveal_remainder)
/// to force a compile-time diagnostic that prints the pretty remainder.
#[doc(hidden)]
#[allow(clippy::panic)]
pub const fn remainder_ctfe_panic<Rem: ConstDesc>() -> usize {
    panic!("{}", Rem::TEXT);
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

const MAX: usize = 1024;

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

const fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

const fn is_ident_continue(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

const fn skip_ident(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && is_ident_continue(b[i]) {
        i += 1;
    }
    i
}

/// `alloc::string::String` → `String`, `core::option::Option<foo::Bar>` → `Option<Bar>`.
const fn write_short_name(buf: &mut [u8], mut pos: usize, s: &str) -> usize {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if is_ident_start(b[i]) {
            let mut last;
            loop {
                last = i;
                i = skip_ident(b, i);
                if i + 1 < b.len() && b[i] == b':' && b[i + 1] == b':' {
                    i += 2;
                } else {
                    break;
                }
            }
            let mut k = last;
            while k < i {
                buf[pos] = b[k];
                pos += 1;
                k += 1;
            }
        } else {
            buf[pos] = b[i];
            pos += 1;
            i += 1;
        }
    }
    pos
}

const fn short_name_len(s: &str) -> usize {
    let mut tmp = [0u8; MAX];
    write_short_name(&mut tmp, 0, s)
}

const fn str_from_buf(buf: &'static [u8; MAX], len: usize) -> &'static str {
    assert!(len <= MAX);
    // Only `write_str` of UTF-8 literals and `type_name` output fills these buffers.
    unsafe { core::str::from_utf8_unchecked(core::slice::from_raw_parts(buf.as_ptr(), len)) }
}

struct Angle2<R, T, const NAME: &'static str>(PhantomData<(R, T)>);
impl<R, T, const NAME: &'static str> Angle2<R, T, NAME> {
    const LEN: usize = NAME.len() + 1 + short_name_len(type_name::<R>()) + 2 + short_name_len(type_name::<T>()) + 1;
    const BUF: [u8; MAX] = {
        let mut buf = [0u8; MAX];
        let mut pos = 0;
        pos = write_str(&mut buf, pos, NAME);
        pos = write_str(&mut buf, pos, "<");
        pos = write_short_name(&mut buf, pos, type_name::<R>());
        pos = write_str(&mut buf, pos, ", ");
        pos = write_short_name(&mut buf, pos, type_name::<T>());
        let _ = write_str(&mut buf, pos, ">");
        buf
    };
    const TEXT: &'static str = str_from_buf(&Self::BUF, Self::LEN);
}

struct Angle1<R, const NAME: &'static str>(PhantomData<R>);
impl<R, const NAME: &'static str> Angle1<R, NAME> {
    const LEN: usize = NAME.len() + 1 + short_name_len(type_name::<R>()) + 1;
    const BUF: [u8; MAX] = {
        let mut buf = [0u8; MAX];
        let mut pos = 0;
        pos = write_str(&mut buf, pos, NAME);
        pos = write_str(&mut buf, pos, "<");
        pos = write_short_name(&mut buf, pos, type_name::<R>());
        let _ = write_str(&mut buf, pos, ">");
        buf
    };
    const TEXT: &'static str = str_from_buf(&Self::BUF, Self::LEN);
}

struct JoinBuf<A, B, const SEP: &'static str>(PhantomData<(A, B)>);
impl<A: ConstDesc, B: ConstDesc, const SEP: &'static str> JoinBuf<A, B, SEP> {
    const LEN: usize = A::TEXT.len() + SEP.len() + B::TEXT.len();
    const BUF: [u8; MAX] = {
        let mut buf = [0u8; MAX];
        let mut pos = 0;
        pos = write_str(&mut buf, pos, A::TEXT);
        pos = write_str(&mut buf, pos, SEP);
        let _ = write_str(&mut buf, pos, B::TEXT);
        buf
    };
    const TEXT: &'static str = str_from_buf(&Self::BUF, Self::LEN);
}

struct WrapBuf<T, const PRE: &'static str, const SUF: &'static str>(PhantomData<T>);
impl<T: ConstDesc, const PRE: &'static str, const SUF: &'static str> WrapBuf<T, PRE, SUF> {
    const LEN: usize = PRE.len() + T::TEXT.len() + SUF.len();
    const BUF: [u8; MAX] = {
        let mut buf = [0u8; MAX];
        let mut pos = 0;
        pos = write_str(&mut buf, pos, PRE);
        pos = write_str(&mut buf, pos, T::TEXT);
        let _ = write_str(&mut buf, pos, SUF);
        buf
    };
    const TEXT: &'static str = str_from_buf(&Self::BUF, Self::LEN);
}

struct ThenBuf<P, S>(PhantomData<(P, S)>);
impl<P: ConstDesc, S: State> ThenBuf<P, S> {
    const LEN: usize = P::TEXT.len() + 4 + S::NAME.len();
    const BUF: [u8; MAX] = {
        let mut buf = [0u8; MAX];
        let mut pos = 0;
        pos = write_str(&mut buf, pos, P::TEXT);
        pos = write_str(&mut buf, pos, " => ");
        let _ = write_str(&mut buf, pos, S::NAME);
        buf
    };
    const TEXT: &'static str = str_from_buf(&Self::BUF, Self::LEN);
}

struct EmptyThenBuf<S: State>(PhantomData<S>);
impl<S: State> EmptyThenBuf<S> {
    const LEN: usize = 3 + S::NAME.len();
    const BUF: [u8; MAX] = {
        let mut buf = [0u8; MAX];
        let mut pos = 0;
        pos = write_str(&mut buf, pos, "=> ");
        let _ = write_str(&mut buf, pos, S::NAME);
        buf
    };
    const TEXT: &'static str = str_from_buf(&Self::BUF, Self::LEN);
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

impl<R, T> ConstDesc for Send<R, T> {
    const TEXT: &'static str = Angle2::<R, T, "Send">::TEXT;
}

impl<R, T> ConstDesc for Call<R, T> {
    const TEXT: &'static str = Angle2::<R, T, "Call">::TEXT;
}

impl<R> ConstDesc for SendAny<R> {
    const TEXT: &'static str = Angle1::<R, "SendAny">::TEXT;
}

impl<T> ConstDesc for Receive<T> {
    const TEXT: &'static str = Angle1::<T, "Receive">::TEXT;
}

impl<T> ConstDesc for Schedule<T> {
    const TEXT: &'static str = Angle1::<T, "Schedule">::TEXT;
}

impl<E: crate::ExternalEffect> ConstDesc for External<E> {
    const TEXT: &'static str = Angle1::<E, "External">::TEXT;
}

impl<E: ConstDesc> ConstDesc for Repeat<E> {
    const TEXT: &'static str = WrapBuf::<E, "Repeat<", ">">::TEXT;
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
        {
            const TEXT: &'static str = JoinBuf::<$H, ($($T,)+), ", ">::TEXT;
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
        {
            const TEXT: &'static str = JoinBuf::<$H, Par<($($T,)+)>, " | ">::TEXT;
        }
        impl<$H: ConstDesc, $($T: ConstDesc),+> ConstDesc for Choice<($H, $($T,)+)>
        where
            Choice<($($T,)+)>: ConstDesc,
        {
            const TEXT: &'static str = JoinBuf::<$H, Choice<($($T,)+)>, " | ">::TEXT;
        }
        impl_const_par!($($T),+);
    };
}

impl_const_seq!(E0, E1, E2, E3, E4, E5, E6, E7, E8, E9);
impl_const_par!(B0, B1, B2, B3, B4, B5, B6, B7, B8, B9);

impl ConstDesc for Par<()> {
    const TEXT: &'static str = "(none)";
}

impl<S: State> ConstDesc for Then<Par<()>, S> {
    const TEXT: &'static str = EmptyThenBuf::<S>::TEXT;
}

impl<P, S: State> ConstDesc for Then<Par<P>, S>
where
    P: Uncons,
    Par<P>: ConstDesc,
{
    const TEXT: &'static str = ThenBuf::<Par<P>, S>::TEXT;
}
