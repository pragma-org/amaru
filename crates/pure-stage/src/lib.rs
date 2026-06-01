// Copyright 2025 PRAGMA
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

#![deny(clippy::future_not_send)]
#![expect(incomplete_features)]
#![feature(generic_const_exprs, specialization)]

mod adapter;
pub mod drop_guard;
mod effect;
mod effect_box;
mod logging;
mod output;
mod receiver;
mod resources;
mod sender;
pub mod serde;
pub mod stage_ref;
mod stagegraph;
mod time;
pub mod tokio;
pub mod trace_buffer;
pub mod trace_match;
mod types;
pub mod typestate;

pub mod simulation;

pub use effect::{
    Effect, Effects, ExternalEffect, ExternalEffectAPI, ScheduleIds, StageResponse, UnknownExternalEffect,
};
pub use output::OutputEffect;
pub use receiver::Receiver;
pub use resources::Resources;
pub use sender::{CallError, Sender};
pub use serde::{
    DeserializerGuard, DeserializerGuards, serialize_external_effect::register_effect_deserializer,
    serialize_send_data::register_data_deserializer,
};
pub use stage_ref::{StageBuildRef, StageRef};
pub use stagegraph::{ScheduleId, StageGraph, StageGraphRunning, stage_name};
pub use time::{Clock, EPOCH, Instant};
pub use trace_buffer::TerminationReason;
pub use trace_match::{
    TraceMatch, assert_trace_contains, assert_trace_does_not_contain, assert_trace_match, tm_add_stage, tm_input,
    tm_send, tm_state, tm_terminate, tm_terminated,
};
pub use types::{BLACKHOLE_NAME, BoxFuture, Name, OrTerminateWith, SendData, TryInStage, Void, err, warn};
pub use typetag;

#[expect(unused)]
mod play {
    use std::marker::PhantomData;

    struct Nil;
    struct Cons<H, T>(PhantomData<(H, T)>);

    trait Sequence {
        type Head;
        type Tail: Sequence;
    }

    impl<H, T: Sequence> Sequence for Cons<H, T> {
        type Head = H;
        type Tail = T;
    }

    impl Sequence for Nil {
        type Head = ();
        type Tail = Nil;
    }

    trait Parallel {
        type Head: Sequence;
        type Tail: Parallel;
    }

    impl<H: Sequence, T: Parallel> Parallel for Cons<H, T> {
        type Head = H;
        type Tail = T;
    }

    impl Parallel for Nil {
        type Head = Nil;
        type Tail = Nil;
    }

    struct Assert<const B: bool>;
    trait IsTrue {}
    impl IsTrue for Assert<true> {}
    trait IsFalse {}
    impl IsFalse for Assert<false> {}

    trait StartsWith<E> {
        const MATCH: bool;
    }

    impl<S: Sequence, E> StartsWith<E> for S {
        default const MATCH: bool = false;
    }

    impl<E, Tail: Sequence> StartsWith<E> for Cons<E, Tail> {
        const MATCH: bool = true;
    }

    struct Found<O>(PhantomData<O>);
    struct NotFound;

    trait TryConsume<E> {
        type Outcome;
    }

    impl<E> TryConsume<E> for Nil {
        type Outcome = NotFound;
    }

    trait TryConsumeWith<E, const M: bool> {
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

    trait PrependOutcome<H> {
        type Outcome;
    }

    impl<H: Sequence, Q: Parallel> PrependOutcome<H> for Found<Q> {
        type Outcome = Found<Cons<H, Q>>;
    }

    impl<H> PrependOutcome<H> for NotFound {
        type Outcome = NotFound;
    }

    trait Consume<E> {
        type Output: Parallel;
    }

    impl<P, E, Out> Consume<E> for P
    where
        P: Parallel + TryConsume<E, Outcome = Found<Out>>,
        Out: Parallel,
    {
        type Output = Out;
    }

    struct X<P>(PhantomData<P>);

    fn consume_parallel<P, E, Out>(_x: X<P>, _e: E) -> X<Out>
    where
        P: Consume<E, Output = Out>,
    {
        X(PhantomData)
    }

    struct EffectA;
    struct EffectB;
    struct EffectC;
    struct EffectZ;

    type MyParallel =
        Cons<Cons<EffectA, Cons<EffectB, Nil>>, Cons<Cons<EffectC, Nil>, Cons<Cons<EffectB, Cons<EffectC, Nil>>, Nil>>>;

    fn run() {
        let x = X::<MyParallel>(PhantomData);
        let x = consume_parallel::<_, EffectA, _>(x, EffectA);
        let x = consume_parallel::<_, EffectC, _>(x, EffectC);
        let x = consume_parallel::<_, EffectB, _>(x, EffectB);
        let x = consume_parallel::<_, EffectB, _>(x, EffectB);
        let x = consume_parallel::<_, EffectC, _>(x, EffectC);

        let y = X::<MyParallel>(PhantomData);
        let y = consume_parallel::<_, EffectC, _>(y, EffectC);

        // This does not compile (no branch starts with EffectZ):
        // let _ = consume_parallel::<MyParallel, EffectZ, _>(X::<MyParallel>(PhantomData));
    }
}
