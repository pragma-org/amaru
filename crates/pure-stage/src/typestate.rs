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

#[macro_use]
pub mod internals {
    use crate::{ExternalEffect, SendData, StageRef};
    use std::{
        any::{Any, type_name},
        fmt,
        marker::PhantomData,
    };

    macro_rules! make_states {
        ($($init:ident),+; $($other:ident),+) => {
            $(
                pub struct $init($crate::typestate::Marker);
                impl $crate::typestate::State for $init {
                    const NAME: &'static str = stringify!($init);
                    const MAKE: fn($crate::typestate::Marker) -> Self = $init;
                    type Initial = $crate::typestate::InitialState;
                }
            )*
            $(
                pub struct $other($crate::typestate::Marker);
                impl $crate::typestate::State for $other {
                    const NAME: &'static str = stringify!($other);
                    const MAKE: fn($crate::typestate::Marker) -> Self = $other;
                    type Initial = $crate::typestate::NotInitialState;
                }
            )*
        };
    }

    macro_rules! effects {
        ($t:ty) => {
            $crate::typestate::Cons<$t, $crate::typestate::Nil>
        };
        ($t:ty, $($eff:ty),*) => {
            $crate::typestate::Cons<$t, effects!($($eff),*)>
        };
    }

    macro_rules! transition {
        ($from:ty => $to:ty: $eff:ty) => {
            impl $crate::typestate::Transition<$to> for $from {
                type Eff = $eff;
            }
        };
    }

    pub struct InitialState;
    pub struct NotInitialState;

    pub struct Marker(Private);
    struct Private;

    pub struct Cons<Head, Tail>(Head, Tail);
    pub struct Nil;

    pub trait Sequence: std::marker::Send {
        type Head;
        type Tail;
        fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result;
    }

    impl<Head, Tail> Sequence for Cons<Head, Tail>
    where
        Head: Effect + std::marker::Send,
        Tail: Sequence,
    {
        type Head = Head;
        type Tail = Tail;
        fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
            Head::fmt(f)?;
            write!(f, ", ")?;
            Tail::fmt(f)
        }
    }
    impl Sequence for Nil {
        type Head = ();
        type Tail = ();
        fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "end")
        }
    }

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

    pub struct Send<T: SendData>(PhantomData<T>);
    impl<T: SendData> Effect for Send<T> {
        fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "Send<{}>", type_name::<T>())
        }
    }
    pub struct Receive<T: SendData>(PhantomData<T>);
    impl<T: SendData> Effect for Receive<T> {
        fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "Receive<{}>", type_name::<T>())
        }
    }
    pub struct Call<T: SendData>(PhantomData<T>);
    impl<T: SendData> Effect for Call<T> {
        fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "Call<{}>", type_name::<T>())
        }
    }
    pub struct Clock();
    impl Effect for Clock {
        fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "Clock")
        }
    }
    pub struct Wait();
    impl Effect for Wait {
        fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "Wait")
        }
    }
    pub struct Schedule<T: SendData>(PhantomData<T>);
    impl<T: SendData> Effect for Schedule<T> {
        fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "Schedule<{}>", type_name::<T>())
        }
    }
    pub struct CancelSchedule();
    impl Effect for CancelSchedule {
        fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "CancelSchedule")
        }
    }
    pub struct External<E: ExternalEffect>(PhantomData<E>);
    impl<E: ExternalEffect> Effect for External<E> {
        fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "External<{}>", type_name::<E>())
        }
    }
    pub struct Terminate();
    impl Effect for Terminate {
        fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "Terminate")
        }
    }
    pub struct AddStage();
    impl Effect for AddStage {
        fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "AddStage")
        }
    }

    pub trait Effect {
        fn fmt(f: &mut fmt::Formatter<'_>) -> fmt::Result;
    }

    pub trait Transition<S: State>: State {
        type Eff: Sequence;
        fn start<M>(self, eff: &crate::Effects<M>) -> Effects<M, Self::Eff, S>
        where
            Self: Sized,
        {
            Effects::new(eff, S::MAKE(Marker(Private)))
        }

        fn to_string() -> String {
            struct D<E: Sequence>(PhantomData<E>);
            impl<E: Sequence> fmt::Display for D<E> {
                fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    E::fmt(f)
                }
            }
            format!(
                "{} -> {} -> {}",
                Self::NAME,
                D::<Self::Eff>(PhantomData),
                S::NAME
            )
        }
    }

    pub struct Effects<M, Eff, S> {
        effects: crate::Effects<M>,
        next: S,
        _ph: PhantomData<(Eff, S)>,
    }

    impl<M, Eff, S: State> Effects<M, Eff, S> {
        fn new(effects: &crate::Effects<M>, next: S) -> Self {
            Self {
                effects: effects.clone(),
                next,
                _ph: PhantomData,
            }
        }

        pub async fn send<T: SendData, E>(self, target: &StageRef<T>, msg: T) -> Effects<M, E, S>
        where
            Eff: Sequence<Head = Send<T>, Tail = E>,
        {
            let send = self.effects.send(target, msg);
            send.await;
            Effects {
                effects: self.effects,
                next: self.next,
                _ph: PhantomData,
            }
        }

        pub fn finish(self) -> S
        where
            Eff: Sequence<Head = (), Tail = ()>,
        {
            self.next
        }
    }
}

use internals::{Cons, InitialState, Marker, Nil, NotInitialState, State, Transition};

#[allow(unused)]
mod chainsync {
    use super::internals::{Receive, Send};

    // First declare the states, with initial state(s) before the semicolon.
    make_states!(Idle; Intersect, Done, CanAwait, MustSend);

    // Then declare the transitions with the effect sequences they require.
    // Note that this is a toy example using the structure of the chainsync
    // protocol, but we don’t have the message types available here.

    transition!(Idle => Intersect: effects!(Send<String>, Send<u8>));
    transition!(Intersect => Idle: effects!(Receive<u8>));

    transition!(Idle => CanAwait: effects!(Send<()>));
    transition!(CanAwait => Idle: effects!(Receive<String>));
    transition!(CanAwait => MustSend: effects!(Receive<()>));
    transition!(MustSend => Idle: effects!(Receive<String>));

    transition!(Idle => Done: effects!(Send<()>));

    /// Illustrate how to hold any current protocol state in a stage.
    ///
    /// Processing the next step will match at runtime on the current state,
    /// then allow only statically specified transitions.
    pub enum Chainsync {
        Idle(Idle),
        Intersect(Intersect),
        Done(Done),
        CanAwait(CanAwait),
        MustSend(MustSend),
    }
}

mod application_code {
    use super::chainsync::*;
    use crate::{StageRef, typestate::internals::Transition};

    /// This illustrates writing a function that performs some protocol step (it could easily
    /// do multiple steps by starting a new transition).
    #[expect(unused)]
    pub async fn intersect(
        // The current state of the protocol.
        s: Idle,
        // Some names (channels) needed to perform the Send effects
        mux: &StageRef<String>,
        other: &StageRef<u8>,
        // THe underlying low-level effects machinery; this could be hidden as well to remove
        // the possibility of performing unchecked effects by passing in the strongly typed
        // `typestate::Effects` instead.
        eff: &crate::Effects<()>,
    ) -> Intersect {
        // Start the transition from the current state. Note that the second type parameter
        // specifies a list of two Send effects with different message types.
        let eff = s.start(eff);
        // Performing the first Send effect consumes the first list element.
        let eff = eff.send(mux, "intersect".to_string()).await;
        // Performing the second Send effect consumes the second list element.
        let eff = eff.send(other, 42).await;
        // Only now, with the effects list empty, can we construct the newly reached state.
        // Application code cannot cheat because all other constructors for `Intersect` are
        // inaccessible.
        eff.finish()
    }

    #[test]
    fn test_intersect() {
        use super::internals::{Transition, initial_state};

        // illustrate how to construct the initial state
        let _s = initial_state::<Idle>();
        // let s = initial_state::<Intersect>();
        assert_eq!(
            // since the state machine is fully declared at the type level, we can e.g.
            // print a transition without having either of the states constructed as a value.
            <Idle as Transition<Intersect>>::to_string(),
            "Idle -> Send<alloc::string::String>, Send<u8>, end -> Intersect"
        );
    }
}
