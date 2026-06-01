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

use std::{fmt, marker::PhantomData, time::Duration};

pub use type_lists::{
    Assert, Cons, Consume, InitialState, IsEmpty, IsFalse, IsTrue, Marker, Nil, NotInitialState, Parallel, Sequence,
    State, initial_state,
};

use crate::{SendData, StageRef};

mod effect;
mod macros;
mod role;
mod type_lists;

pub mod prelude {
    pub use super::effect::{
        AddStage, Call, CancelSchedule, Clock, External, Receive, Repeat, Schedule, Send, Terminate, Wait,
    };
}

pub trait Transition<S: State>: State {
    type Eff: Parallel;
    fn start<M>(self, eff: crate::Effects<M>) -> Behaviour<M, Self::Eff, S>
    where
        Self: Sized,
    {
        Behaviour::new(eff, S::MAKE(Marker(type_lists::Private)))
    }

    fn to_string() -> String {
        struct D<E: Parallel>(PhantomData<E>);
        impl<E: Parallel> fmt::Display for D<E> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                E::fmt(f)
            }
        }
        format!("{} -> {} -> {}", Self::NAME, D::<Self::Eff>(PhantomData), S::NAME)
    }
}

pub struct Behaviour<M, Eff, S> {
    effects: crate::Effects<M>,
    next: S,
    _ph: PhantomData<(Eff, S)>,
}

impl<M, Eff, S: State> Behaviour<M, Eff, S> {
    fn new(effects: crate::Effects<M>, next: S) -> Self {
        Self { effects, next, _ph: PhantomData }
    }

    pub async fn send<T: SendData, E>(self, target: &StageRef<T>, msg: T) -> Behaviour<M, E, S>
    where
        Eff: Consume<prelude::Send<T>, Output = E>,
    {
        let send = self.effects.send(target, msg);
        send.await;
        Behaviour { effects: self.effects, next: self.next, _ph: PhantomData }
    }

    pub fn receive<T: SendData, E>(self, _msg: &T) -> Behaviour<M, E, S>
    where
        Eff: Consume<prelude::Receive<T>, Output = E>,
    {
        Behaviour { effects: self.effects, next: self.next, _ph: PhantomData }
    }

    pub async fn wait<E>(self, delay: Duration) -> Behaviour<M, E, S>
    where
        Eff: Consume<prelude::Wait, Output = E>,
    {
        self.effects.wait(delay).await;
        Behaviour { effects: self.effects, next: self.next, _ph: PhantomData }
    }

    // pub fn unroll<E: Effect, const MIN: usize, const MAX: usize, Tail>(
    //     self,
    // ) -> Behaviour<M, Cons<E, Cons<prelude::Repeat<E, { MIN.saturating_sub(1) }, { sub_one(MAX) }>, Tail>>, S>
    // where
    //     Eff: Consume<prelude::Repeat<E, MIN, MAX>, Output = Tail>,
    // {
    //     let _x = const { MAX.checked_sub(1).expect("MAX must be positive") };

    //     Behaviour { effects: self.effects, next: self.next, _ph: PhantomData }
    // }

    // pub fn end_loop<E: Effect, Tail, const MAX: usize>(self) -> Behaviour<M, Tail, S>
    // where
    //     Eff: Consume<prelude::Repeat<E, 0, MAX>, Output = Tail>,
    // {
    //     Behaviour { effects: self.effects, next: self.next, _ph: PhantomData }
    // }

    pub fn finish(self) -> S
    where
        Eff: IsEmpty,
        Assert<{ Eff::EMPTY }>: IsTrue,
    {
        self.next
    }
}

#[cfg(clippy)]
#[expect(clippy::panic)]
pub const fn sub_one(n: usize) -> usize {
    if n > 0 {
        n - 1
    } else {
        panic!("MAX must be positive; use `cargo build` to get error location.");
    }
}
#[cfg(not(clippy))]
pub const fn sub_one(n: usize) -> usize {
    n.saturating_sub(1)
}

#[cfg(test)]
mod tests;
