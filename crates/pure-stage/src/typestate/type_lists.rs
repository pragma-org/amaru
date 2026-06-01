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

use std::{any::Any, fmt, marker::PhantomData};

use crate::typestate::effect::Effect;

pub struct InitialState;
pub struct NotInitialState;

pub struct Marker(pub(super) Private);
pub(super) struct Private;

pub trait State: Any + std::marker::Send {
    const NAME: &'static str;
    const MAKE: fn(Marker) -> Self;
    type Initial;
}

pub fn initial_state<S>() -> S
where
    S: State<Initial = InitialState>,
{
    S::MAKE(Marker(Private))
}

pub struct Nil;
pub struct Cons<H, T>(PhantomData<(H, T)>);

pub trait Sequence: IsEmpty {
    type Head;
    type Tail: Sequence;
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result;
}

impl<H: Effect, T: Sequence + IsEmpty> Sequence for Cons<H, T> {
    type Head = H;
    type Tail = T;
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        H::fmt(f)?;
        if !T::EMPTY {
            write!(f, ", ")?;
            T::fmt(f)?;
        }
        Ok(())
    }
}

impl Sequence for Nil {
    type Head = ();
    type Tail = Nil;
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "end")
    }
}

pub trait Parallel: IsEmpty {
    type Head: Sequence;
    type Tail: Parallel;
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result;
}

impl<H: Sequence, T: Parallel + IsEmpty> Parallel for Cons<H, T> {
    type Head = H;
    type Tail = T;
    fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
        H::fmt(f)?;
        if !T::EMPTY {
            write!(f, " | ")?;
            T::fmt(f)?;
        }
        Ok(())
    }
}

impl Parallel for Nil {
    type Head = Nil;
    type Tail = Nil;
    fn fmt(_f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Ok(())
    }
}

pub struct Assert<const B: bool>;
pub trait IsTrue {}
impl IsTrue for Assert<true> {}
pub trait IsFalse {}
impl IsFalse for Assert<false> {}

pub trait StartsWith<E> {
    const MATCH: bool;
}

impl<S: Sequence, E> StartsWith<E> for S {
    default const MATCH: bool = false;
}

impl<E: Effect, Tail: Sequence> StartsWith<E> for Cons<E, Tail> {
    const MATCH: bool = true;
}

pub struct Found<O>(PhantomData<O>);
pub struct NotFound;

pub trait TryConsume<E> {
    type Outcome;
}

impl<E> TryConsume<E> for Nil {
    type Outcome = NotFound;
}

pub trait TryConsumeWith<E, const M: bool> {
    type Outcome;
}

impl<E, PHead: Sequence + StartsWith<E>, PTail: Parallel> TryConsume<E> for Cons<PHead, PTail>
where
    Cons<PHead, PTail>: TryConsumeWith<E, { <PHead as StartsWith<E>>::MATCH }>,
{
    type Outcome = <Cons<PHead, PTail> as TryConsumeWith<E, { <PHead as StartsWith<E>>::MATCH }>>::Outcome;
}

impl<E, PHead: Sequence + StartsWith<E>, PTail: Parallel> TryConsumeWith<E, true> for Cons<PHead, PTail>
where
    Assert<{ <PHead as StartsWith<E>>::MATCH }>: IsTrue,
{
    type Outcome = Found<Cons<PHead::Tail, PTail>>;
}

impl<E, PHead: Sequence + StartsWith<E>, PTail: Parallel> TryConsumeWith<E, false> for Cons<PHead, PTail>
where
    Assert<{ <PHead as StartsWith<E>>::MATCH }>: IsFalse,
    PTail: Parallel + TryConsume<E>,
    <PTail as TryConsume<E>>::Outcome: PrependOutcome<PHead>,
{
    type Outcome = <<PTail as TryConsume<E>>::Outcome as PrependOutcome<PHead>>::Outcome;
}

pub trait PrependOutcome<H> {
    type Outcome;
}

impl<H: Sequence, Q: Parallel> PrependOutcome<H> for Found<Q> {
    type Outcome = Found<Cons<H, Q>>;
}

impl<H> PrependOutcome<H> for NotFound {
    type Outcome = NotFound;
}

// --- Cleanup of empty Sequences after Consume ------------------------------

pub trait IsEmpty {
    const EMPTY: bool;
}

impl IsEmpty for Nil {
    const EMPTY: bool = true;
}

impl<H, T> IsEmpty for Cons<H, T> {
    const EMPTY: bool = false;
}

/// Recursively removes all `Cons<Nil, _>` nodes from a Parallel spine.
pub trait Clean {
    type Cleaned: Parallel;
}

impl Clean for Nil {
    type Cleaned = Nil;
}

impl<H: Sequence, T: Parallel + Clean> Clean for Cons<H, T>
where
    // Select the concrete CleanWith<false/true> impl via the same
    // const-generic where-clause dispatch used by TryConsume/TryConsumeWith.
    Cons<H, T>: CleanWith<{ <H as IsEmpty>::EMPTY }>,
{
    type Cleaned = <Cons<H, T> as CleanWith<{ <H as IsEmpty>::EMPTY }>>::Cleaned;
}

pub trait CleanWith<const E: bool> {
    type Cleaned: Parallel + IsEmpty;
}

// Head is a non-empty Sequence → keep the node, recursively clean the tail.
impl<H: Sequence, T: Parallel + Clean> CleanWith<false> for Cons<H, T> {
    type Cleaned = Cons<H, T::Cleaned>;
}

// Head is the empty sequence Nil → drop the node entirely.
impl<T: Parallel + Clean> CleanWith<true> for Cons<Nil, T> {
    type Cleaned = T::Cleaned;
}

pub trait Consume<E> {
    type Output: Parallel;
}

impl<P, E, Out> Consume<E> for P
where
    P: Parallel + TryConsume<E, Outcome = Found<Out>>,
    Out: Parallel + Clean,
{
    // The Parallel returned by Consume is always cleaned of any
    // Sequence branches that became empty as a result of the consumption.
    type Output = <Out as Clean>::Cleaned;
}
