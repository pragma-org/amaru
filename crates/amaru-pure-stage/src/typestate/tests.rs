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

//! Type-level remainder tests. Runtime driving of `Session` lives in
//! `tests/typestate.rs`.

use super::{
    Cons, Here, Nil, OnReceive, Select, There, To,
    effect::Send,
    list::{Clean, describe},
    session::describe_receive,
};

trait TypeEq<T> {}
impl<T> TypeEq<T> for T {}

fn assert_types_eq<A, B>()
where
    A: TypeEq<B>,
{
}

mod toy {
    use crate::typestate::prelude::*;

    make_states!(pub Live { Idle; Intersect, Done, CanAwait, MustReply });

    pub struct Peer;

    on_receive!(Idle, FindIntersect => Send<Peer, String>, Send<Peer, u8> => Intersect | Wait => Idle);
    on_receive!(Intersect, u8 => Idle);
    on_receive!(Idle, RequestNext => Send<Peer, ()> => CanAwait);
    on_receive!(CanAwait, String => Idle);
    on_receive!(CanAwait, AwaitReply => MustReply);
    on_receive!(MustReply, String => Idle);
    on_receive!(Idle, ClientDone => Send<Peer, ()> => Done);

    pub struct FindIntersect;
    pub struct RequestNext;
    pub struct AwaitReply;
    pub struct ClientDone;
}

#[test]
fn receive_constructor_selects_by_input_variant() {
    use toy::*;
    assert_eq!(
        describe_receive::<Idle, FindIntersect>(),
        format!(
            "Idle + Receive<{}> → Send<{}, alloc::string::String>, Send<{}, u8> => Intersect | Wait => Idle",
            std::any::type_name::<FindIntersect>(),
            std::any::type_name::<Peer>(),
            std::any::type_name::<Peer>(),
        )
    );
    assert_eq!(
        describe_receive::<Idle, ClientDone>(),
        format!(
            "Idle + Receive<{}> → Send<{}, ()> => Done",
            std::any::type_name::<ClientDone>(),
            std::any::type_name::<Peer>()
        )
    );
    assert_eq!(
        describe_receive::<CanAwait, AwaitReply>(),
        format!("CanAwait + Receive<{}> → => MustReply", std::any::type_name::<AwaitReply>())
    );
}

#[test]
fn select_picks_first_matching_head() {
    type Rem = Cons<Cons<Send<toy::Peer, u8>, To<toy::Idle>>, Cons<Cons<Send<toy::Peer, String>, To<toy::Done>>, Nil>>;

    assert_types_eq::<
        <Rem as Select<Send<toy::Peer, u8>, Here>>::Rest,
        Cons<To<toy::Idle>, Cons<Cons<Send<toy::Peer, String>, To<toy::Done>>, Nil>>,
    >();

    assert_types_eq::<
        <Rem as Select<Send<toy::Peer, String>, There<Here>>>::Rest,
        Cons<Cons<Send<toy::Peer, u8>, To<toy::Idle>>, Cons<To<toy::Done>, Nil>>,
    >();
}

#[test]
fn clean_drops_exhausted_sequences() {
    type Dirty = Cons<Nil, Cons<To<toy::Done>, Nil>>;
    assert_types_eq::<<Dirty as Clean>::Out, Cons<To<toy::Done>, Nil>>();
}

#[test]
fn describe_remainder() {
    type Rem = <toy::Idle as OnReceive<toy::FindIntersect>>::Then;
    assert_eq!(
        describe::<Rem>(),
        format!(
            "Send<{}, alloc::string::String>, Send<{}, u8> => Intersect | Wait => Idle",
            std::any::type_name::<toy::Peer>(),
            std::any::type_name::<toy::Peer>()
        )
    );
}

#[test]
fn initial_state_wraps_into_live_enum() {
    let live: toy::Live = super::initial_state::<toy::Idle>().into();
    assert!(matches!(live, toy::Live::Idle(_)));
}
