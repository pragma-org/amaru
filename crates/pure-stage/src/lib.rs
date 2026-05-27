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

    // Empty list marker
    struct Nil;

    // Cons cell for both Sequence and Parallel
    struct Cons<H, T>(PhantomData<(H, T)>);

    // ─────────────────────────────────────────────────────────────
    // 1. Sequence = a single chain of effect types
    trait Sequence {
        type Head;
        type Tail: Sequence; // Nil or another Cons
    }

    impl<H, T: Sequence> Sequence for Cons<H, T> {
        type Head = H;
        type Tail = T;
    }

    impl Sequence for Nil {
        type Head = (); // dummy, never used
        type Tail = Nil;
    }

    // ─────────────────────────────────────────────────────────────
    // 2. Parallel = a list of Sequences
    trait Parallel {
        type Head: Sequence;
        type Tail: Parallel;
    }

    impl<H: Sequence, T: Parallel> Parallel for Cons<H, T> {
        type Head = H;
        type Tail = T;
    }

    impl Parallel for Nil {
        type Head = Nil; // dummy
        type Tail = Nil;
    }

    // ─────────────────────────────────────────────────────────────
    // 3. The operation we want: consume the first Sequence whose Head matches E
    trait Consume<E> {
        type Output: Parallel; // new Parallel list with the matched Head removed
    }

    impl<E, PHead: Sequence, PTail: Parallel> Consume<E> for Cons<PHead, PTail>
    where
        PTail: Parallel + Consume<E>,
    {
        default type Output = Cons<PHead, PTail::Output>;
    }

    trait Matches<E> {}
    impl<E, Tail: Sequence> Matches<E> for Cons<E, Tail> {}

    impl<E, PHead: Sequence, PTail: Parallel> Consume<E> for Cons<PHead, PTail>
    where
        PTail: Parallel + Consume<E>,
        PHead: Matches<E>,
    {
        type Output = Cons<PHead::Tail, PTail>;
    }

    // trait Consume<E, N: Num> {
    //     type Output: Parallel; // new Parallel list with the matched Head removed
    // }
    // struct S<T>(T);
    // struct Z;
    // trait Num {}
    // impl Num for Z {}
    // impl<T: Num> Num for S<T> {}

    // impl<E, PHead: Sequence, PTail: Parallel, N: Num> Consume<E, S<N>> for Cons<PHead, PTail>
    // where
    //     PTail: Parallel + Consume<E, N>,
    // {
    //     default type Output = Cons<PHead, PTail::Output>;
    // }

    // impl<E, STail: Sequence, PTail: Parallel> Consume<E, Z> for Cons<Cons<E, STail>, PTail>
    // where
    //     PTail: Parallel,
    //     E: Sized,
    // {
    //     type Output = Cons<STail, PTail>;
    // }

    // ─────────────────────────────────────────────────────────────
    // Usage example
    struct X<P>(PhantomData<P>);

    fn consume_parallel<P, E, Out>(_: X<P>) -> X<Out>
    where
        P: Consume<E, Output = Out>,
    {
        // the real implementation would do whatever runtime work you need
        X(PhantomData)
    }

    // Example types
    struct EffectA;
    struct EffectB;
    struct EffectC;

    // A Parallel list: [ [A, B], [C], [B, C] ]
    type MyParallel =
        Cons<Cons<EffectA, Cons<EffectB, Nil>>, Cons<Cons<EffectC, Nil>, Cons<Cons<EffectB, Cons<EffectC, Nil>>, Nil>>>;

    // fn run() {
    //     let x = X::<MyParallel>(PhantomData);
    //     // This compiles: finds the first Sequence whose Head == EffectA and consumes it
    //     let x = consume_parallel::<_, EffectA, _>(x);
    //     let x = consume_parallel::<_, EffectB, _>(x);
    // }

    // This does NOT compile (type error): no Sequence starts with EffectZ
    // let _ = consume_parallel::<MyParallel, EffectZ, _>(/* … */);
}
