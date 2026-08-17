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

//! Type-level remainder algebra.
//!
//! A remainder is a choice of [`Then<P, S>`] (`A => S | B => T | C => U`).
//! `P` is a flat [`Cons`] of sequences (parallel, one `S` for the `Then`).
//! [`Select`]`<E, I>` takes the **leftmost** matching head; `I` is inferred
//! and is unique (later matches are not offered). Exclusive choice drops
//! every `Then` that was not chosen.
//!
//! [`Repeat<Seq>`](super::Repeat) is a Kleene star. Selecting `Seq`'s first
//! effect **unrolls** the rest in front of the same `Repeat` (or keeps a
//! single-effect star). If that first step does not match, the star is
//! **discarded** (zero iterations) and `E` is taken from what follows — the
//! rest of this sequence, then later parallel branches. If both the star and
//! the suffix match, the star wins (it is to the left).
//!
//! [`CanFinish`]: strip leading `Repeat` on each parallel branch, drop empties,
//! succeed iff nothing remains.
//!
//! **Limits:** lists are flat, not tree-associative. Sequences are ordered.
//! Two choice alternatives with the same head are ambiguous (see [`Select`]
//! `There` on `Then`). `finish` only strips `Repeat` at a branch prefix.
//! [`StripRepeat`] knows `Send`, `SendAny`, `Wait`, `Terminate`.

use std::{fmt, marker::PhantomData};

use super::{
    Effect, State,
    effect::{Repeat, SendAny},
};

pub struct Nil;
pub struct Cons<H, T>(PhantomData<(H, T)>);

/// Parallel composition `P` of sequences, then next state `S`.
pub struct Then<P, S>(PhantomData<(P, S)>);

/// Index of the first matching branch.
pub struct Here;
/// Index one past `I`.
pub struct There<I>(PhantomData<I>);
/// Search inside the first choice alternative with inner index `I`.
pub struct In<I>(PhantomData<I>);
/// Discard a leading [`Repeat`](super::Repeat) and search what follows with `I`.
pub struct Skip<I>(PhantomData<I>);

/// First effect of a `Repeat` body (a single tag, or the head of a `Cons`).
pub trait FirstEffect {
    type Head;
}

impl<R, T> FirstEffect for super::effect::Send<R, T> {
    type Head = super::effect::Send<R, T>;
}

impl<R> FirstEffect for SendAny<R> {
    type Head = SendAny<R>;
}

impl FirstEffect for super::effect::Wait {
    type Head = super::effect::Wait;
}

impl FirstEffect for super::effect::Terminate {
    type Head = super::effect::Terminate;
}

impl<T> FirstEffect for super::effect::Call<T> {
    type Head = super::effect::Call<T>;
}

impl FirstEffect for super::effect::Clock {
    type Head = super::effect::Clock;
}

impl<T> FirstEffect for super::effect::Schedule<T> {
    type Head = super::effect::Schedule<T>;
}

impl FirstEffect for super::effect::CancelSchedule {
    type Head = super::effect::CancelSchedule;
}

impl<E: crate::ExternalEffect> FirstEffect for super::effect::External<E> {
    type Head = super::effect::External<E>;
}

impl FirstEffect for super::effect::AddStage {
    type Head = super::effect::AddStage;
}

impl<T> FirstEffect for super::effect::Receive<T> {
    type Head = super::effect::Receive<T>;
}

impl<H, T> FirstEffect for Cons<H, T> {
    type Head = H;
}

/// Sequence heads that are not a [`Repeat`] (later parallel branches apply).
pub trait NotRepeat {}

impl<R, T> NotRepeat for super::effect::Send<R, T> {}
impl<R> NotRepeat for SendAny<R> {}
impl NotRepeat for super::effect::Wait {}
impl NotRepeat for super::effect::Terminate {}
impl<T> NotRepeat for super::effect::Call<T> {}
impl NotRepeat for super::effect::Clock {}
impl<T> NotRepeat for super::effect::Schedule<T> {}
impl NotRepeat for super::effect::CancelSchedule {}
impl<E: crate::ExternalEffect> NotRepeat for super::effect::External<E> {}
impl NotRepeat for super::effect::AddStage {}
impl<T> NotRepeat for super::effect::Receive<T> {}

/// Compile-time inequality for `Select` bounds. Names are compared only
/// within one rustc invocation, so two distinct types never collide.
const fn types_eq<A, B>() -> bool {
    let a = core::any::type_name::<A>().as_bytes();
    let b = core::any::type_name::<B>().as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

pub struct If<const B: bool>;

pub trait IsFalse {}
impl IsFalse for If<false> {}

/// Select the leftmost head `E`. `I` is inferred and unique.
pub trait Select<E, I> {
    type Rest;
}

impl<E, Tail, Rest> Select<E, Here> for Cons<Cons<E, Tail>, Rest>
where
    E: NotRepeat,
{
    type Rest = Cons<Tail, Rest>;
}

/// `Repeat` at the front of a sequence: keep or unroll.
impl<E, Seq, Tail, Rest> Select<E, Here> for Cons<Cons<Repeat<Seq>, Tail>, Rest>
where
    Self: TakeRepeat<E, Here>,
{
    type Rest = <Self as TakeRepeat<E, Here>>::Rest;
}

/// `Repeat` that does not match `E`: discard it and search what follows.
impl<E, Seq, Tail, Rest, I> Select<E, Skip<I>> for Cons<Cons<Repeat<Seq>, Tail>, Rest>
where
    Self: TakeRepeat<E, Skip<I>>,
{
    type Rest = <Self as TakeRepeat<E, Skip<I>>>::Rest;
}

/// Later parallel sequence. Applies only when this head cannot serve `E`.
impl<E, Eff, Tail, Rest, I> Select<E, There<I>> for Cons<Cons<Eff, Tail>, Rest>
where
    Eff: NotRepeat,
    If<{ types_eq::<Eff, E>() }>: IsFalse,
    Rest: Select<E, I>,
{
    type Rest = Cons<Cons<Eff, Tail>, Rest::Rest>;
}

impl<E, I, P, S> Select<E, I> for Then<P, S>
where
    P: Select<E, I>,
    P::Rest: Clean,
{
    type Rest = Then<<P::Rest as Clean>::Out, S>;
}

/// First choice alternative that can serve `E`. Other `Then`s are dropped.
impl<E, I, P, S, Rest> Select<E, In<I>> for Cons<Then<P, S>, Rest>
where
    P: Select<E, I>,
    P::Rest: Clean,
{
    type Rest = Cons<Then<<P::Rest as Clean>::Out, S>, Nil>;
}

/// Later choice alternative. This `Then` is dropped.
///
/// `There` stays a candidate even while `E` is still inferred (a `types_eq`
/// bound here would freeze `T` to the first alternative's payload). Distinct
/// heads therefore pick a unique `I`; two alternatives with the same head
/// are ambiguous.
impl<E, I, P, S, Rest> Select<E, There<I>> for Cons<Then<P, S>, Rest>
where
    Rest: Select<E, I>,
{
    type Rest = Rest::Rest;
}

/// Concatenate two sequences.
pub trait Concat<Suf> {
    type Out;
}

impl<Suf> Concat<Suf> for Nil {
    type Out = Suf;
}

impl<H, T: Concat<Suf>, Suf> Concat<Suf> for Cons<H, T> {
    type Out = Cons<H, T::Out>;
}

/// How a leading [`Repeat`] serves a selection of `E`.
///
/// - [`Here`]: `Repeat<E>` keeps the star; `Repeat<Cons<E, T>>` unrolls `T`
///   in front of the same `Repeat`.
/// - [`Skip`]: the star's first step is not `E`, so it is discarded and `E`
///   is selected from what follows.
pub trait TakeRepeat<E, I> {
    type Rest;
}

impl<E, Tail, Rest> TakeRepeat<E, Here> for Cons<Cons<Repeat<E>, Tail>, Rest> {
    type Rest = Cons<Cons<Repeat<E>, Tail>, Rest>;
}

impl<E, T, Tail, Rest> TakeRepeat<E, Here> for Cons<Cons<Repeat<Cons<E, T>>, Tail>, Rest>
where
    T: Concat<Cons<Repeat<Cons<E, T>>, Tail>>,
{
    type Rest = Cons<T::Out, Rest>;
}

impl<E, Seq, Tail, Rest, I> TakeRepeat<E, Skip<I>> for Cons<Cons<Repeat<Seq>, Tail>, Rest>
where
    Seq: FirstEffect,
    If<{ types_eq::<Seq::Head, E>() }>: IsFalse,
    Cons<Tail, Rest>: Clean,
    <Cons<Tail, Rest> as Clean>::Out: Select<E, I>,
{
    type Rest = <<Cons<Tail, Rest> as Clean>::Out as Select<E, I>>::Rest;
}

/// Strip leading [`Repeat`] from a sequence.
pub trait StripRepeat {
    type Out;
}

impl StripRepeat for Nil {
    type Out = Nil;
}

impl<E, T: StripRepeat> StripRepeat for Cons<Repeat<E>, T> {
    type Out = T::Out;
}

impl<R, T, Tail> StripRepeat for Cons<super::effect::Send<R, T>, Tail> {
    type Out = Cons<super::effect::Send<R, T>, Tail>;
}

impl<R, Tail> StripRepeat for Cons<SendAny<R>, Tail> {
    type Out = Cons<SendAny<R>, Tail>;
}

impl<Tail> StripRepeat for Cons<super::effect::Wait, Tail> {
    type Out = Cons<super::effect::Wait, Tail>;
}

impl<Tail> StripRepeat for Cons<super::effect::Terminate, Tail> {
    type Out = Cons<super::effect::Terminate, Tail>;
}

/// After stripping `Repeat` prefixes, drop empty branches.
pub trait Prune {
    type Out;
}

impl Prune for Nil {
    type Out = Nil;
}

impl<Seq, Rest: Prune> Prune for Cons<Seq, Rest>
where
    Seq: StripRepeat,
    Seq::Out: ConsIfPresent<Rest::Out>,
{
    type Out = <Seq::Out as ConsIfPresent<Rest::Out>>::Out;
}

pub trait ConsIfPresent<Rest> {
    type Out;
}

impl<Rest> ConsIfPresent<Rest> for Nil {
    type Out = Rest;
}

impl<H, T, Rest> ConsIfPresent<Rest> for Cons<H, T> {
    type Out = Cons<Cons<H, T>, Rest>;
}

/// A remainder that may [`Session::finish`](super::Session::finish) in `S`.
///
/// For [`Then<P, S>`], leading [`Repeat`] on each branch of `P` is discarded;
/// empty branches are dropped; finish is allowed only when no branch remains.
pub trait CanFinish<S, I> {}

impl<P, S: State> CanFinish<S, Here> for Then<P, S> where P: Prune<Out = Nil> {}

impl<P, S: State, Rest> CanFinish<S, Here> for Cons<Then<P, S>, Rest> where Then<P, S>: CanFinish<S, Here> {}

impl<P, S: State, Rest, I> CanFinish<S, There<I>> for Cons<Then<P, S>, Rest> where Rest: CanFinish<S, I> {}

/// Drop exhausted (`Nil`) sequences after a consume.
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

impl<P: Clean, S> Clean for Then<P, S> {
    type Out = Then<P::Out, S>;
}

impl<P, S, Rest: Clean> Clean for Cons<Then<P, S>, Rest>
where
    Then<P, S>: Clean,
{
    type Out = Cons<<Then<P, S> as Clean>::Out, Rest::Out>;
}

/// Format a sequence (`Send<A>, Wait`).
pub trait FmtSeq {
    fn fmt_seq(f: &mut fmt::Formatter<'_>) -> fmt::Result;
}

/// Format a parallel / choice remainder.
pub trait FmtPar {
    fn fmt_par(f: &mut fmt::Formatter<'_>) -> fmt::Result;
}

pub trait IsNil {
    const IS_NIL: bool;
}

impl IsNil for Nil {
    const IS_NIL: bool = true;
}

impl<H, T> IsNil for Cons<H, T> {
    const IS_NIL: bool = false;
}

impl<H, T> crate::typestate::effect::Effect for Repeat<Cons<H, T>>
where
    Cons<H, T>: FmtSeq,
{
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Repeat<")?;
        <Cons<H, T> as FmtSeq>::fmt_seq(f)?;
        write!(f, ">")
    }
}

impl<E: Effect> FmtSeq for Cons<E, Nil> {
    fn fmt_seq(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        E::fmt(f)
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

impl<P: FmtPar + IsNil, S: State> FmtSeq for Then<P, S> {
    fn fmt_seq(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if P::IS_NIL {
            write!(f, "=> {}", S::NAME)
        } else {
            P::fmt_par(f)?;
            write!(f, " => {}", S::NAME)
        }
    }
}

impl<P: FmtPar + IsNil, S: State> FmtPar for Then<P, S> {
    fn fmt_par(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        <Self as FmtSeq>::fmt_seq(f)
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
