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
    Cons, Here, Nil, OnReceive, Select, Then, There,
    effect::{Repeat, Send, SendAny},
    list::{CanFinish, Clean, describe},
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
    type Left = Then<Cons<Cons<Send<toy::Peer, u8>, Nil>, Nil>, toy::Idle>;
    type Rem = Cons<Left, Cons<Then<Cons<Cons<Send<toy::Peer, String>, Nil>, Nil>, toy::Done>, Nil>>;

    assert_types_eq::<<Rem as Select<Send<toy::Peer, u8>, super::list::Left<Here>>>::Rest, Cons<Then<Nil, toy::Idle>, Nil>>();
}

#[test]
fn clean_drops_exhausted_sequences() {
    type Dirty = Cons<Nil, Cons<Then<Nil, toy::Done>, Nil>>;
    assert_types_eq::<<Dirty as Clean>::Out, Cons<Then<Nil, toy::Done>, Nil>>();
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
fn unused_star_does_not_block_finish() {
    type Rem = Cons<Then<Cons<Cons<Repeat<SendAny<toy::Peer>>, Nil>, Nil>, toy::Idle>, Nil>;
    fn assert_finish<R: CanFinish<toy::Idle, Here>>() {}
    assert_finish::<Rem>();
}

#[test]
fn star_then_required_send_does_not_finish() {
    type Rem =
        Cons<Then<Cons<Cons<Repeat<SendAny<toy::Peer>>, Cons<Send<toy::Peer, u8>, Nil>>, Nil>, toy::Idle>, Nil>;
    fn assert_selects_send<R: Select<Send<toy::Peer, u8>, Here>>() {}
    assert_selects_send::<Rem>();
}

#[test]
fn using_star_keeps_the_star() {
    type Inner = Then<Cons<Cons<Repeat<SendAny<toy::Peer>>, Nil>, Nil>, toy::Idle>;
    type Rem = Cons<Inner, Nil>;
    assert_types_eq::<<Rem as Select<SendAny<toy::Peer>, Here>>::Rest, Rem>();
}

#[test]
fn parallel_star_is_usable_beside_a_required_send() {
    type Rem = Cons<
        Then<Cons<Cons<Send<toy::Peer, u8>, Nil>, Cons<Cons<Repeat<SendAny<toy::Peer>>, Nil>, Nil>>, toy::Idle>,
        Nil,
    >;
    fn assert_send<R: Select<Send<toy::Peer, u8>, Here>>() {}
    fn assert_any<R: Select<SendAny<toy::Peer>, There<Here>>>() {}
    assert_send::<Rem>();
    assert_any::<Rem>();
}

/// `Send<A, A1>, Send<B, B1> => StateA | Send<C, C1>, Send<D, D1> => StateC`
#[allow(dead_code)]
mod exclusive_choice {
    use super::*;
    use crate::typestate::prelude::*;

    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct A1;
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct B1;
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct C1;
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct D1;

    define_role_tag!(pub RoleA);
    define_role_tag!(pub RoleB);
    define_role_tag!(pub RoleC);
    define_role_tag!(pub RoleD);

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Go;

    make_states!(pub Live { Idle; StateA, StateC });
    on_receive!(Idle as IdleIn {
        Go => { Send<RoleA, A1>, Send<RoleB, B1> => StateA | Send<RoleC, C1>, Send<RoleD, D1> => StateC }
    });

    type Rem = <Idle as OnReceive<Go>>::Then;
    type AfterA = <Rem as Select<Send<RoleA, A1>, crate::typestate::list::Left<Here>>>::Rest;
    type AfterC = <Rem as Select<Send<RoleC, C1>, crate::typestate::list::Right<Here>>>::Rest;

    #[test]
    fn after_a_only_b_remains() {
        fn assert_b<R: Select<Send<RoleB, B1>, Here>>() {}
        assert_b::<AfterA>();
        assert_eq!(
            describe::<AfterA>(),
            format!("Send<{}, {}> => StateA", std::any::type_name::<RoleB>(), std::any::type_name::<B1>())
        );
    }

    #[test]
    fn after_c_only_d_remains() {
        fn assert_d<R: Select<Send<RoleD, D1>, Here>>() {}
        assert_d::<AfterC>();
        assert_eq!(
            describe::<AfterC>(),
            format!("Send<{}, {}> => StateC", std::any::type_name::<RoleD>(), std::any::type_name::<D1>())
        );
    }
}

#[test]
fn initial_state_wraps_into_live_enum() {
    let live: toy::Live = super::initial_state::<toy::Idle>().into();
    assert!(matches!(live, toy::Live::Idle(_)));
}

#[allow(dead_code)]
mod convert {
    use crate::typestate::prelude::*;

    make_states!(Live { Idle; Done });

    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    struct Ping(u8);
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    struct Pong(u16);

    define_mailbox!(Mail { Ping(Ping), Pong(Pong) });
    on_receive!(Idle as IdleIn { Ping => { Done } });
    on_receive!(Done as DoneIn {});

    #[test]
    fn convert_input_rejects_inadmissible_mailbox() {
        let idle = initial_state::<Idle>();
        match idle.convert_input(Mail::Pong(Pong(1))) {
            Ok(IdleIn::Ping(_)) => panic!("Pong must not convert in Idle"),
            Err(Mail::Pong(Pong(1))) => {}
            Err(other) => panic!("unexpected mailbox {other:?}"),
        }
    }
}
