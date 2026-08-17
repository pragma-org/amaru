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

//! Type-level lists and first-match selection without specialization.
//!
//! A remainder is a *parallel* list (`Cons<seq, Cons<seq, Nil>>`) of *sequences*.
//! Each sequence is `Cons<Effect, …>` ending in [`To<S>`](super::To).
//!
//! [`Select`]`<E, I>` picks the first sequence whose head is `E`. The index `I`
//! ([`Here`] / [`There`]) is inferred, the same trick as frunk's `Selector`.
//! Distinct heads on sibling branches never overlap; two branches that start
//! with the same effect make `I` ambiguous, which is the desired compile error.

use std::{fmt, marker::PhantomData};

use super::{Effect, State, To};

pub struct Nil;
pub struct Cons<H, T>(PhantomData<(H, T)>);

/// Index of the first alternative in a parallel list.
pub struct Here;
/// Index one past `I` in a parallel list.
pub struct There<I>(PhantomData<I>);

/// Select the alternative whose head is `E`. `I` is inferred.
pub trait Select<E, I> {
    type Rest;
}

impl<E, Tail, Rest> Select<E, Here> for Cons<Cons<E, Tail>, Rest> {
    type Rest = Cons<Tail, Rest>;
}

impl<S: State, Rest> Select<To<S>, Here> for Cons<To<S>, Rest> {
    type Rest = Rest;
}

impl<E, Head, Rest, I> Select<E, There<I>> for Cons<Head, Rest>
where
    Rest: Select<E, I>,
{
    type Rest = Cons<Head, Rest::Rest>;
}

/// Drop exhausted (`Nil`) alternatives after a consume.
pub trait Clean {
    type Out;
}

impl Clean for Nil {
    type Out = Nil;
}

impl<T: Clean> Clean for Cons<Nil, T> {
    type Out = T::Out;
}

impl<H, Tail, T: Clean> Clean for Cons<Cons<H, Tail>, T> {
    type Out = Cons<Cons<H, Tail>, T::Out>;
}

impl<S: State, T: Clean> Clean for Cons<To<S>, T> {
    type Out = Cons<To<S>, T::Out>;
}

/// Format a sequence (`Send<A>, Wait => Idle`).
pub trait FmtSeq {
    fn fmt_seq(f: &mut fmt::Formatter<'_>) -> fmt::Result;
}

/// Format a parallel remainder (`A => S | B => T`).
pub trait FmtPar {
    fn fmt_par(f: &mut fmt::Formatter<'_>) -> fmt::Result;
}

impl<S: State> FmtSeq for To<S> {
    fn fmt_seq(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "=> {}", S::NAME)
    }
}

impl<E: Effect, S: State> FmtSeq for Cons<E, To<S>> {
    fn fmt_seq(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        E::fmt(f)?;
        write!(f, " => {}", S::NAME)
    }
}

impl<E: Effect, H, T> FmtSeq for Cons<E, Cons<H, T>>
where
    Cons<H, T>: FmtSeq,
{
    fn fmt_seq(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        E::fmt(f)?;
        write!(f, ", ")?;
        <Cons<H, T> as FmtSeq>::fmt_seq(f)
    }
}

impl FmtPar for Nil {
    fn fmt_par(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(none)")
    }
}

impl<H: FmtSeq> FmtPar for Cons<H, Nil> {
    fn fmt_par(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        H::fmt_seq(f)
    }
}

impl<H: FmtSeq, T1, T2> FmtPar for Cons<H, Cons<T1, T2>>
where
    Cons<T1, T2>: FmtPar,
{
    fn fmt_par(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        H::fmt_seq(f)?;
        write!(f, " | ")?;
        <Cons<T1, T2> as FmtPar>::fmt_par(f)
    }
}

/// Render a remainder as a string (for tests and diagnostics).
pub fn describe<R: FmtPar>() -> String {
    struct D<R>(PhantomData<R>);
    impl<R: FmtPar> fmt::Display for D<R> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            R::fmt_par(f)
        }
    }
    D::<R>(PhantomData).to_string()
}
