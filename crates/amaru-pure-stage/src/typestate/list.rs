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

//! Type-level lists for remainders.
//!
//! A remainder is a *choice* of [`Then`] alternatives (`A => S | B => T`).
//! Each [`Then<P, S>`] is a parallel composition `P` of sequences plus a single
//! next state `S`. Every parallel branch must be discharged before
//! [`Session::finish`](super::Session::finish): leading [`Repeat`] is stripped,
//! empty branches are dropped, and finish is allowed only when nothing remains.

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
/// Choice: search the left [`Then`].
pub struct Left<I>(PhantomData<I>);
/// Choice: search the right [`Then`].
pub struct Right<I>(PhantomData<I>);

/// Select the branch whose head is `E`. `I` is inferred.
pub trait Select<E, I> {
    type Rest;
}

impl<E, Tail, Rest> Select<E, Here> for Cons<Cons<E, Tail>, Rest> {
    type Rest = Cons<Tail, Rest>;
}

/// Using a starred permission does not consume the star.
impl<R, Tail, Rest> Select<SendAny<R>, Here> for Cons<Cons<Repeat<SendAny<R>>, Tail>, Rest> {
    type Rest = Cons<Cons<Repeat<SendAny<R>>, Tail>, Rest>;
}

/// Look through a leading star to a required `Send` sequenced after it.
impl<Tag, T, F, H, U, Rest> Select<super::effect::Send<Tag, T>, Here> for Cons<Cons<Repeat<F>, Cons<H, U>>, Rest>
where
    Cons<Cons<H, U>, Rest>: Select<super::effect::Send<Tag, T>, Here>,
    <Cons<Cons<H, U>, Rest> as Select<super::effect::Send<Tag, T>, Here>>::Rest: PrependSeq<Repeat<F>>,
{
    type Rest =
        <<Cons<Cons<H, U>, Rest> as Select<super::effect::Send<Tag, T>, Here>>::Rest as PrependSeq<Repeat<F>>>::Out;
}

/// Put `H` back at the front of the first sequence.
pub trait PrependSeq<H> {
    type Out;
}

impl<H, Seq, Rest> PrependSeq<H> for Cons<Seq, Rest> {
    type Out = Cons<Cons<H, Seq>, Rest>;
}

impl<E, Eff, Tail, Rest, I> Select<E, There<I>> for Cons<Cons<Eff, Tail>, Rest>
where
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

impl<E, I, P, S> Select<E, I> for Cons<Then<P, S>, Nil>
where
    P: Select<E, I>,
    P::Rest: Clean,
{
    type Rest = Cons<Then<<P::Rest as Clean>::Out, S>, Nil>;
}

impl<E, I, P, S, Q, T> Select<E, Left<I>> for Cons<Then<P, S>, Cons<Then<Q, T>, Nil>>
where
    P: Select<E, I>,
    P::Rest: Clean,
{
    type Rest = Cons<Then<<P::Rest as Clean>::Out, S>, Nil>;
}

impl<E, I, P, S, Q, T> Select<E, Right<I>> for Cons<Then<P, S>, Cons<Then<Q, T>, Nil>>
where
    Q: Select<E, I>,
    Q::Rest: Clean,
{
    type Rest = Cons<Then<<Q::Rest as Clean>::Out, T>, Nil>;
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

impl<H, Rest, S, I> CanFinish<S, There<I>> for Cons<H, Rest> where Rest: CanFinish<S, I> {}

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
