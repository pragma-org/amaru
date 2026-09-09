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
//! A remainder is a [`Choice`] of [`Then<Par<P>, S>`] (`A => S | B => T | C => U`).
//! [`Par`]`<P>` is a tuple of sequences; each sequence is a tuple of effects
//! (or [`Repeat`](super::Repeat) of a tuple). rustc therefore prints
//! `Choice<(Then<Par<((Send<Role, T>, Wait),)>, Idle>,)>` rather than a Cons
//! encoding. [`Select`]`<E, I>` takes the **leftmost** matching head; `I` is
//! inferred and is unique (later matches are not offered). Exclusive choice
//! drops every `Then` that was not chosen.
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
//! **Limits:** sequences, parallel branches, and choice alternatives are tuples
//! of length at most 10. Sequences are ordered. Two choice alternatives with
//! the same head are ambiguous (see [`Select`] `There` on [`Choice`]). `finish`
//! only strips `Repeat` at a branch prefix. [`SetTimeout`](super::SetTimeout) /
//! [`ClearTimeout`](super::ClearTimeout) are required steps and are not
//! stripped.

use std::{fmt, marker::PhantomData};

use super::{
    Effect, State,
    effect::{Repeat, SendAny},
};

/// Exclusive choice of [`Then`] alternatives. `C` is a tuple, at most 10 long.
pub struct Choice<C>(PhantomData<C>);

/// Parallel composition of sequences. `P` is a tuple of sequence tuples, at most 10 long.
pub struct Par<P>(PhantomData<P>);

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

/// Split a tuple into its first element and the rest.
///
/// Implemented for arities 1 through 10. `()` and 11-element tuples have no
/// impl, so exceeding the remainder limit is a trait-bound error.
#[diagnostic::on_unimplemented(
    message = "session remainder tuples support at most 10 elements",
    note = "`{Self}` is empty or longer than 10 — split the protocol or shorten this list"
)]
pub trait Uncons {
    type Head;
    type Tail;
}

/// Prepend `H` to this tuple. Implemented for arities 0 through 9 (result ≤ 10).
pub trait Prefix<H> {
    type Out;
}

/// Body of a [`Repeat`]: a single [`NotRepeat`] effect, or a tuple of effects.
pub trait RepeatBody {
    type Head;
    type Tail;
}

/// Concatenate two sequences. Generated per arity so recursion is structural.
pub trait Concat<Suf> {
    type Out;
}

/// [`Select`] on a tuple of sequences (the inner type of [`Par`]).
pub trait SelectTup<E, I> {
    type Rest;
}

/// Strip stars and drop empty sequences from a parallel tuple.
pub trait PruneTup {
    type Out;
}

macro_rules! impl_tuple_ladders {
    ($H:ident $(, $T:ident)* $(,)?) => {
        impl<$H $(, $T)*> Uncons for ($H, $($T,)*) {
            type Head = $H;
            type Tail = ($($T,)*);
        }
        impl<$H $(, $T)*> RepeatBody for ($H, $($T,)*) {
            type Head = $H;
            type Tail = ($($T,)*);
        }
        impl<$H $(, $T)*, Suf> Concat<Suf> for ($H, $($T,)*)
        where
            ($($T,)*): Concat<Suf>,
            <($($T,)*) as Concat<Suf>>::Out: Prefix<$H>,
        {
            type Out = <<($($T,)*) as Concat<Suf>>::Out as Prefix<$H>>::Out;
        }
        impl_tuple_ladders!($($T),*);
    };
    () => {};
}

macro_rules! impl_prefix {
    (@from [$($T:ident),*]) => {
        impl<H $(, $T)*> Prefix<H> for ($($T,)*) {
            type Out = (H, $($T,)*);
        }
    };
    (@acc [$($done:ident),*] $next:ident $(, $rest:ident)*) => {
        impl_prefix!(@from [$($done),*]);
        impl_prefix!(@acc [$($done,)* $next] $($rest),*);
    };
    (@acc [$($done:ident),*]) => {
        impl_prefix!(@from [$($done),*]);
    };
    ($($T:ident),*) => {
        impl_prefix!(@acc [] $($T),*);
    };
}

impl_tuple_ladders!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9);
impl_prefix!(T0, T1, T2, T3, T4, T5, T6, T7, T8);

impl<Suf> Concat<Suf> for () {
    type Out = Suf;
}

impl PruneTup for () {
    type Out = ();
}

/// Sequence heads that are not a [`Repeat`] (later parallel branches apply).
pub trait NotRepeat {}

impl<R, T> NotRepeat for super::effect::Send<R, T> {}
impl<R> NotRepeat for SendAny<R> {}
impl NotRepeat for super::effect::Wait {}
impl NotRepeat for super::effect::Terminate {}
impl<R, T> NotRepeat for super::effect::Call<R, T> {}
impl NotRepeat for super::effect::Clock {}
impl<T> NotRepeat for super::effect::Schedule<T> {}
impl NotRepeat for super::effect::CancelSchedule {}
impl NotRepeat for super::effect::SetTimeout {}
impl NotRepeat for super::effect::ClearTimeout {}
impl<E: crate::ExternalEffect> NotRepeat for super::effect::External<E> {}
impl NotRepeat for super::effect::AddStage {}
impl<T> NotRepeat for super::effect::Receive<T> {}

impl<E: NotRepeat> RepeatBody for E {
    type Head = E;
    type Tail = ();
}

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
#[diagnostic::on_unimplemented(
    message = "cannot `{E}` from this remainder",
    label = "not allowed in the remaining session",
    note = "`{Self}` has no leftmost `{E}`"
)]
pub trait Select<E, I> {
    type Rest;
}

/// Dispatch `Select` on the head of the first parallel sequence.
///
/// `Self` is that head (`E` or [`Repeat<Body>`]), so the Repeat/effect cases
/// are disjoint type constructors rather than overlapping `Par<P>` impls.
pub trait TakeHead<E, I, SeqTail, RestPar> {
    type Rest;
}

#[diagnostic::do_not_recommend]
impl<E: NotRepeat, SeqTail, RestPar> TakeHead<E, Here, SeqTail, RestPar> for E
where
    SeqTail: ConsIfPresent<RestPar>,
{
    type Rest = SeqTail::Out;
}

#[diagnostic::do_not_recommend]
impl<E, Body, SeqTail, RestPar> TakeHead<E, Here, SeqTail, RestPar> for Repeat<Body>
where
    Body: RepeatBody<Head = E>,
    Repeat<Body>: UnrollRepeat<Body, SeqTail>,
    <Repeat<Body> as UnrollRepeat<Body, SeqTail>>::Out: ConsIfPresent<RestPar>,
{
    type Rest = <<Repeat<Body> as UnrollRepeat<Body, SeqTail>>::Out as ConsIfPresent<RestPar>>::Out;
}

#[diagnostic::do_not_recommend]
impl<E, Body, SeqTail, RestPar, I> TakeHead<E, Skip<I>, SeqTail, RestPar> for Repeat<Body>
where
    Body: RepeatBody,
    If<{ types_eq::<Body::Head, E>() }>: IsFalse,
    SeqTail: ConsIfPresent<RestPar>,
    SeqTail::Out: SelectTup<E, I>,
{
    type Rest = <SeqTail::Out as SelectTup<E, I>>::Rest;
}

#[diagnostic::do_not_recommend]
impl<E, Eff, SeqTail, RestPar, I> TakeHead<E, There<I>, SeqTail, RestPar> for Eff
where
    Eff: NotRepeat,
    If<{ types_eq::<Eff, E>() }>: IsFalse,
    SeqTail: Prefix<Eff>,
    RestPar: SelectTup<E, I>,
    <RestPar as SelectTup<E, I>>::Rest: Prefix<SeqTail::Out>,
{
    type Rest = <<RestPar as SelectTup<E, I>>::Rest as Prefix<SeqTail::Out>>::Out;
}

#[diagnostic::do_not_recommend]
impl<E, I, P> Select<E, I> for Par<P>
where
    P: SelectTup<E, I>,
{
    type Rest = Par<P::Rest>;
}

#[diagnostic::do_not_recommend]
impl<E, I, P, S> Select<E, I> for Then<Par<P>, S>
where
    Par<P>: Select<E, I>,
    <Par<P> as Select<E, I>>::Rest: Clean,
{
    type Rest = Then<<<Par<P> as Select<E, I>>::Rest as Clean>::Out, S>;
}

/// First choice alternative that can serve `E`. Other `Then`s are dropped.
///
/// `There` stays a candidate even while `E` is still inferred (a `types_eq`
/// bound here would freeze `T` to the first alternative's payload). Distinct
/// heads therefore pick a unique `I`; two alternatives with the same head
/// are ambiguous.
macro_rules! impl_choice_select {
    ($H:ident $(, $T:ident)* $(,)?) => {
        #[diagnostic::do_not_recommend]
        impl<E, I, $H $(, $T)*> Select<E, In<I>> for Choice<($H, $($T,)*)>
        where
            $H: Select<E, I>,
            <$H as Select<E, I>>::Rest: Clean,
        {
            type Rest = Choice<(<<$H as Select<E, I>>::Rest as Clean>::Out,)>;
        }
        impl_choice_select!(@there $H $(, $T)*);
        impl_choice_select!($($T),*);
    };
    (@there $H:ident $(, $T:ident)+) => {
        #[diagnostic::do_not_recommend]
        impl<E, I, $H, $($T),+> Select<E, There<I>> for Choice<($H, $($T,)+)>
        where
            Choice<($($T,)+)>: Select<E, I>,
        {
            type Rest = <Choice<($($T,)+)> as Select<E, I>>::Rest;
        }
    };
    (@there $H:ident) => {};
    () => {};
}

impl_choice_select!(C0, C1, C2, C3, C4, C5, C6, C7, C8, C9);

/// Unroll `Repeat<Body>` in front of `Suffix`: leftover body, then the star, then `Suffix`.
pub trait UnrollRepeat<Body, Suffix> {
    type Out;
}

impl<Body, Suffix> UnrollRepeat<Body, Suffix> for Repeat<Body>
where
    Body: RepeatBody,
    (Repeat<Body>,): Concat<Suffix>,
    Body::Tail: Concat<<(Repeat<Body>,) as Concat<Suffix>>::Out>,
{
    type Out = <Body::Tail as Concat<<(Repeat<Body>,) as Concat<Suffix>>::Out>>::Out;
}

/// Drop a leading [`Repeat`] from the current sequence, leaving the suffix.
///
/// Used by [`SessionOps::discard_repeat`](super::SessionOps::discard_repeat). There is
/// no impl when the head is not a star, so skipping is a compile error.
#[diagnostic::on_unimplemented(
    message = "no leading Repeat to discard in `{Self}`",
    label = "remainder does not start with Repeat"
)]
pub trait DiscardRepeat {
    type Out;
}

#[diagnostic::do_not_recommend]
impl<P, Body> DiscardRepeat for Par<P>
where
    P: Uncons,
    P::Head: Uncons<Head = Repeat<Body>>,
    <P::Head as Uncons>::Tail: ConsIfPresent<P::Tail>,
    Par<<<P::Head as Uncons>::Tail as ConsIfPresent<P::Tail>>::Out>: Clean,
{
    type Out = <Par<<<P::Head as Uncons>::Tail as ConsIfPresent<P::Tail>>::Out> as Clean>::Out;
}

#[diagnostic::do_not_recommend]
impl<P, S> DiscardRepeat for Then<Par<P>, S>
where
    Par<P>: DiscardRepeat,
{
    type Out = Then<<Par<P> as DiscardRepeat>::Out, S>;
}

#[diagnostic::do_not_recommend]
impl<C> DiscardRepeat for Choice<C>
where
    C: Uncons,
    C::Head: DiscardRepeat,
    C::Tail: Prefix<<C::Head as DiscardRepeat>::Out>,
{
    type Out = Choice<<C::Tail as Prefix<<C::Head as DiscardRepeat>::Out>>::Out>;
}

/// How a leading effect or [`Repeat`] is treated when stripping stars for [`CanFinish`].
pub trait StripHead<Tail> {
    type Out;
}

impl<Seq, Tail: StripRepeat> StripHead<Tail> for Repeat<Seq> {
    type Out = Tail::Out;
}

impl<R, T, Tail: Prefix<super::effect::Send<R, T>>> StripHead<Tail> for super::effect::Send<R, T> {
    type Out = Tail::Out;
}
impl<R, Tail: Prefix<SendAny<R>>> StripHead<Tail> for SendAny<R> {
    type Out = Tail::Out;
}
impl<Tail: Prefix<super::effect::Wait>> StripHead<Tail> for super::effect::Wait {
    type Out = Tail::Out;
}
impl<Tail: Prefix<super::effect::Terminate>> StripHead<Tail> for super::effect::Terminate {
    type Out = Tail::Out;
}
impl<R, T, Tail: Prefix<super::effect::Call<R, T>>> StripHead<Tail> for super::effect::Call<R, T> {
    type Out = Tail::Out;
}
impl<Tail: Prefix<super::effect::Clock>> StripHead<Tail> for super::effect::Clock {
    type Out = Tail::Out;
}
impl<T, Tail: Prefix<super::effect::Schedule<T>>> StripHead<Tail> for super::effect::Schedule<T> {
    type Out = Tail::Out;
}
impl<Tail: Prefix<super::effect::CancelSchedule>> StripHead<Tail> for super::effect::CancelSchedule {
    type Out = Tail::Out;
}
impl<Tail: Prefix<super::effect::SetTimeout>> StripHead<Tail> for super::effect::SetTimeout {
    type Out = Tail::Out;
}
impl<Tail: Prefix<super::effect::ClearTimeout>> StripHead<Tail> for super::effect::ClearTimeout {
    type Out = Tail::Out;
}
impl<E: crate::ExternalEffect, Tail: Prefix<super::effect::External<E>>> StripHead<Tail>
    for super::effect::External<E>
{
    type Out = Tail::Out;
}
impl<Tail: Prefix<super::effect::AddStage>> StripHead<Tail> for super::effect::AddStage {
    type Out = Tail::Out;
}
impl<T, Tail: Prefix<super::effect::Receive<T>>> StripHead<Tail> for super::effect::Receive<T> {
    type Out = Tail::Out;
}

/// Strip leading [`Repeat`] from a sequence.
pub trait StripRepeat {
    type Out;
}

impl StripRepeat for () {
    type Out = ();
}

/// After stripping `Repeat` prefixes, drop empty branches.
pub trait Prune {
    type Out;
}

impl<P: PruneTup> Prune for Par<P> {
    type Out = Par<P::Out>;
}

pub trait ConsIfPresent<Rest> {
    type Out;
}

impl<Rest> ConsIfPresent<Rest> for () {
    type Out = Rest;
}

impl<Seq, Rest> ConsIfPresent<Rest> for Seq
where
    Seq: Uncons,
    Rest: Prefix<Seq>,
{
    type Out = Rest::Out;
}

macro_rules! impl_seq_tuples {
    ($H:ident $(, $T:ident)* $(,)?) => {
        impl<$H $(, $T)*> StripRepeat for ($H, $($T,)*)
        where
            $H: StripHead<($($T,)*)>,
        {
            type Out = <$H as StripHead<($($T,)*)>>::Out;
        }
        impl<E, I, $H $(, $T)*> SelectTup<E, I> for ($H, $($T,)*)
        where
            $H: Uncons,
            <$H as Uncons>::Head: TakeHead<E, I, <$H as Uncons>::Tail, ($($T,)*)>,
        {
            type Rest = <<$H as Uncons>::Head as TakeHead<E, I, <$H as Uncons>::Tail, ($($T,)*)>>::Rest;
        }
        impl<$H $(, $T)*> PruneTup for ($H, $($T,)*)
        where
            $H: StripRepeat,
            ($($T,)*): PruneTup,
            <$H as StripRepeat>::Out: ConsIfPresent<<($($T,)*) as PruneTup>::Out>,
        {
            type Out = <<$H as StripRepeat>::Out as ConsIfPresent<<($($T,)*) as PruneTup>::Out>>::Out;
        }
        impl_seq_tuples!($($T),*);
    };
    () => {};
}

impl_seq_tuples!(S0, S1, S2, S3, S4, S5, S6, S7, S8, S9);

/// A remainder that may [`SessionOps::finish`](super::SessionOps::finish) in `S`.
///
/// For [`Then<Par<P>, S>`], leading [`Repeat`] on each branch of `P` is discarded;
/// empty branches are dropped; finish is allowed only when no branch remains.
#[diagnostic::on_unimplemented(
    message = "cannot finish in `{S}` from remainder `{Self}`",
    label = "required effects still remain",
    note = "leading Repeat is stripped automatically; other effects must be performed first"
)]
pub trait CanFinish<S, I> {}

#[diagnostic::do_not_recommend]
impl<P, S: State> CanFinish<S, Here> for Then<Par<P>, S> where Par<P>: Prune<Out = Par<()>> {}

macro_rules! impl_choice_finish {
    ($H:ident $(, $T:ident)* $(,)?) => {
        #[diagnostic::do_not_recommend]
        impl<S: State, $H $(, $T)*> CanFinish<S, Here> for Choice<($H, $($T,)*)>
        where
            $H: CanFinish<S, Here>,
        {
        }
        impl_choice_finish!(@there $H $(, $T)*);
        impl_choice_finish!($($T),*);
    };
    (@there $H:ident $(, $T:ident)+) => {
        #[diagnostic::do_not_recommend]
        impl<S: State, I, $H, $($T),+> CanFinish<S, There<I>> for Choice<($H, $($T,)+)>
        where
            Choice<($($T,)+)>: CanFinish<S, I>,
        {
        }
    };
    (@there $H:ident) => {};
    () => {};
}

impl_choice_finish!(C0, C1, C2, C3, C4, C5, C6, C7, C8, C9);

/// [`Select`] plus [`Clean`]. `I` is inferred and unique.
///
/// [`Session`](super::Session) methods name this in the **return type**, not a
/// `where` clause, so a missing effect is E0277 rather than E0599 (“no method”).
#[diagnostic::on_unimplemented(
    message = "cannot `{E}` from this remainder",
    label = "not allowed in the remaining session",
    note = "`{Self}` has no leftmost `{E}`"
)]
pub trait Take<E, I> {
    type Rest;
}

#[diagnostic::do_not_recommend]
impl<R, E, I> Take<E, I> for R
where
    R: Select<E, I>,
    R::Rest: Clean,
{
    type Rest = <R::Rest as Clean>::Out;
}

/// [`CanFinish`] as a projection so [`SessionOps::finish`](super::SessionOps::finish)
/// does not hide behind E0599.
#[diagnostic::on_unimplemented(
    message = "cannot finish in `{S}` from remainder `{Self}`",
    label = "required effects still remain",
    note = "leading Repeat is stripped automatically; other effects must be performed first"
)]
pub trait FinishIn<S, I> {
    type Out;
}

#[diagnostic::do_not_recommend]
impl<R, S: State, I> FinishIn<S, I> for R
where
    R: CanFinish<S, I>,
{
    type Out = S;
}

/// Drop exhausted sequences after a consume. Selection already omits empty
/// branches, so this is identity on [`Par`] / [`Then`] / [`Choice`].
pub trait Clean {
    type Out;
}

impl<P> Clean for Par<P> {
    type Out = Par<P>;
}

impl<P, S> Clean for Then<Par<P>, S> {
    type Out = Then<Par<P>, S>;
}

impl<C> Clean for Choice<C> {
    type Out = Choice<C>;
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

impl IsNil for Par<()> {
    const IS_NIL: bool = true;
}

impl<P: Uncons> IsNil for Par<P> {
    const IS_NIL: bool = false;
}

impl FmtSeq for () {
    fn fmt_seq(_f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Ok(())
    }
}

macro_rules! impl_fmt_seq {
    ($H:ident) => {
        impl<$H: Effect> FmtSeq for ($H,) {
            fn fmt_seq(f: &mut fmt::Formatter<'_>) -> fmt::Result {
                $H::fmt(f)
            }
        }
        impl<$H: Effect> crate::typestate::effect::Effect for Repeat<($H,)> {
            fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "Repeat<")?;
                $H::fmt(f)?;
                write!(f, ">")
            }
        }
    };
    ($H:ident, $($T:ident),+) => {
        impl<$H: Effect, $($T: Effect),+> FmtSeq for ($H, $($T,)+) {
            fn fmt_seq(f: &mut fmt::Formatter<'_>) -> fmt::Result {
                $H::fmt(f)?;
                write!(f, ", ")?;
                <($($T,)+) as FmtSeq>::fmt_seq(f)
            }
        }
        impl<$H: Effect, $($T: Effect),+> crate::typestate::effect::Effect for Repeat<($H, $($T,)+)> {
            fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "Repeat<")?;
                <($H, $($T,)+) as FmtSeq>::fmt_seq(f)?;
                write!(f, ">")
            }
        }
        impl_fmt_seq!($($T),+);
    };
}

macro_rules! impl_fmt_par {
    ($H:ident) => {
        impl<$H: FmtSeq> FmtPar for Par<($H,)> {
            fn fmt_par(f: &mut fmt::Formatter<'_>) -> fmt::Result {
                $H::fmt_seq(f)
            }
        }
        impl<$H: FmtPar> FmtPar for Choice<($H,)> {
            fn fmt_par(f: &mut fmt::Formatter<'_>) -> fmt::Result {
                $H::fmt_par(f)
            }
        }
    };
    ($H:ident, $($T:ident),+) => {
        impl<$H: FmtSeq, $($T: FmtSeq),+> FmtPar for Par<($H, $($T,)+)> {
            fn fmt_par(f: &mut fmt::Formatter<'_>) -> fmt::Result {
                $H::fmt_seq(f)?;
                write!(f, " | ")?;
                <Par<($($T,)+)> as FmtPar>::fmt_par(f)
            }
        }
        impl<$H: FmtPar, $($T: FmtPar),+> FmtPar for Choice<($H, $($T,)+)> {
            fn fmt_par(f: &mut fmt::Formatter<'_>) -> fmt::Result {
                $H::fmt_par(f)?;
                write!(f, " | ")?;
                <Choice<($($T,)+)> as FmtPar>::fmt_par(f)
            }
        }
        impl_fmt_par!($($T),+);
    };
}

impl_fmt_seq!(E0, E1, E2, E3, E4, E5, E6, E7, E8, E9);
impl_fmt_par!(B0, B1, B2, B3, B4, B5, B6, B7, B8, B9);

impl FmtPar for Par<()> {
    fn fmt_par(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(none)")
    }
}

impl<P, S: State> FmtSeq for Then<Par<P>, S>
where
    Par<P>: FmtPar + IsNil,
{
    fn fmt_seq(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if <Par<P> as IsNil>::IS_NIL {
            write!(f, "=> {}", S::NAME)
        } else {
            <Par<P> as FmtPar>::fmt_par(f)?;
            write!(f, " => {}", S::NAME)
        }
    }
}

impl<P, S: State> FmtPar for Then<Par<P>, S>
where
    Par<P>: FmtPar + IsNil,
{
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
