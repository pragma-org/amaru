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
    Cons, FmtPar, Here, Nil, OnReceive, Select, Then,
    effect::{ClearTimeout, Repeat, Send, SendAny, SetTimeout},
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

fn assert_after<R, E, Expect, I>()
where
    R: Select<E, I>,
    R::Rest: TypeEq<Expect>,
{
}

fn describe_after<R, E, I>() -> String
where
    R: Select<E, I>,
    R::Rest: FmtPar,
{
    describe::<R::Rest>()
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

    assert_after::<Rem, Send<toy::Peer, u8>, Cons<Then<Nil, toy::Idle>, Nil>, _>();
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
fn set_timeout_is_required_before_finish() {
    type Rem = Cons<Then<Cons<Cons<SetTimeout, Nil>, Nil>, toy::Idle>, Nil>;
    fn assert_selects<R: Select<SetTimeout, I>, I>() {}
    assert_selects::<Rem, _>();
    assert_eq!(describe::<Rem>(), "SetTimeout => Idle");
}

#[test]
fn clear_timeout_is_required_before_finish() {
    type Rem = Cons<Then<Cons<Cons<ClearTimeout, Nil>, Nil>, toy::Idle>, Nil>;
    fn assert_selects<R: Select<ClearTimeout, I>, I>() {}
    assert_selects::<Rem, _>();
    assert_eq!(describe::<Rem>(), "ClearTimeout => Idle");
}

#[test]
fn unused_star_does_not_block_finish() {
    type Rem = Cons<Then<Cons<Cons<Repeat<SendAny<toy::Peer>>, Nil>, Nil>, toy::Idle>, Nil>;
    fn assert_finish<R: CanFinish<toy::Idle, Here>>() {}
    assert_finish::<Rem>();
}

#[test]
fn star_then_required_send_does_not_finish() {
    type Rem = Cons<Then<Cons<Cons<Repeat<SendAny<toy::Peer>>, Cons<Send<toy::Peer, u8>, Nil>>, Nil>, toy::Idle>, Nil>;
    fn assert_selects_send<R: Select<Send<toy::Peer, u8>, I>, I>() {}
    assert_selects_send::<Rem, _>();
}

#[test]
fn using_star_keeps_the_star() {
    type Inner = Then<Cons<Cons<Repeat<SendAny<toy::Peer>>, Nil>, Nil>, toy::Idle>;
    type Rem = Cons<Inner, Nil>;
    assert_after::<Rem, SendAny<toy::Peer>, Rem, _>();
}

#[test]
fn parallel_star_is_usable_beside_a_required_send() {
    type Rem = Cons<
        Then<Cons<Cons<Send<toy::Peer, u8>, Nil>, Cons<Cons<Repeat<SendAny<toy::Peer>>, Nil>, Nil>>, toy::Idle>,
        Nil,
    >;
    fn assert_send<R: Select<Send<toy::Peer, u8>, I>, I>() {}
    fn assert_any<R: Select<SendAny<toy::Peer>, I>, I>() {}
    assert_send::<Rem, _>();
    assert_any::<Rem, _>();
}

#[test]
fn repeat_sequence_unrolls_then_keeps_the_star() {
    type Seq = Cons<Send<toy::Peer, u8>, Cons<Send<toy::Peer, u16>, Nil>>;
    type Rem = Cons<Then<Cons<Cons<Repeat<Seq>, Nil>, Nil>, toy::Idle>, Nil>;
    type Expect = Cons<Then<Cons<Cons<Send<toy::Peer, u16>, Cons<Repeat<Seq>, Nil>>, Nil>, toy::Idle>, Nil>;
    assert_after::<Rem, Send<toy::Peer, u8>, Expect, _>();
}

#[test]
fn repeat_is_discarded_when_the_suffix_matches() {
    type Rem =
        Cons<Then<Cons<Cons<Repeat<Send<toy::Peer, u8>>, Cons<Send<toy::Peer, u16>, Nil>>, Nil>, toy::Idle>, Nil>;
    assert_after::<Rem, Send<toy::Peer, u16>, Cons<Then<Nil, toy::Idle>, Nil>, _>();
}

#[test]
fn repeat_of_sequence_is_discarded_when_the_suffix_matches() {
    type Seq = Cons<Send<toy::Peer, u8>, Cons<Send<toy::Peer, u16>, Nil>>;
    type Rem = Cons<Then<Cons<Cons<Repeat<Seq>, Cons<Send<toy::Peer, u32>, Nil>>, Nil>, toy::Idle>, Nil>;
    assert_after::<Rem, Send<toy::Peer, u32>, Cons<Then<Nil, toy::Idle>, Nil>, _>();
}

#[test]
fn repeat_wins_over_a_matching_suffix() {
    type Rem = Cons<Then<Cons<Cons<Repeat<Send<toy::Peer, u8>>, Cons<Send<toy::Peer, u8>, Nil>>, Nil>, toy::Idle>, Nil>;
    assert_after::<Rem, Send<toy::Peer, u8>, Rem, _>();
}

#[test]
fn leftmost_parallel_head_wins_when_both_match() {
    type Rem = Cons<
        Then<
            Cons<Cons<Send<toy::Peer, u8>, Nil>, Cons<Cons<Send<toy::Peer, u8>, Cons<Send<toy::Peer, u16>, Nil>>, Nil>>,
            toy::Idle,
        >,
        Nil,
    >;
    type Expect = Cons<Then<Cons<Cons<Send<toy::Peer, u8>, Cons<Send<toy::Peer, u16>, Nil>>, Nil>, toy::Idle>, Nil>;
    assert_after::<Rem, Send<toy::Peer, u8>, Expect, _>();
    assert_eq!(
        describe_after::<Rem, Send<toy::Peer, u8>, _>(),
        format!(
            "Send<{}, u8>, Send<{}, u16> => Idle",
            std::any::type_name::<toy::Peer>(),
            std::any::type_name::<toy::Peer>()
        )
    );
}

#[test]
fn star_macro_unrolls_the_sequence() {
    use crate::typestate::prelude::*;
    type Rem = Cons<Then<Cons<Cons<star!(Send<toy::Peer, u8>, Send<toy::Peer, u16>), Nil>, Nil>, toy::Idle>, Nil>;
    type Expect = Cons<
        Then<
            Cons<Cons<Send<toy::Peer, u16>, Cons<star!(Send<toy::Peer, u8>, Send<toy::Peer, u16>), Nil>>, Nil>,
            toy::Idle,
        >,
        Nil,
    >;
    assert_after::<Rem, Send<toy::Peer, u8>, Expect, _>();
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
    type AfterAExpect = Cons<Then<Cons<Cons<Send<RoleB, B1>, Nil>, Nil>, StateA>, Nil>;
    type AfterCExpect = Cons<Then<Cons<Cons<Send<RoleD, D1>, Nil>, Nil>, StateC>, Nil>;

    #[test]
    fn after_a_only_b_remains() {
        assert_after::<Rem, Send<RoleA, A1>, AfterAExpect, _>();
        fn assert_b<R: Select<Send<RoleB, B1>, I>, I>() {}
        assert_b::<AfterAExpect, _>();
        assert_eq!(
            describe_after::<Rem, Send<RoleA, A1>, _>(),
            format!("Send<{}, {}> => StateA", std::any::type_name::<RoleB>(), std::any::type_name::<B1>())
        );
    }

    #[test]
    fn after_c_only_d_remains() {
        assert_after::<Rem, Send<RoleC, C1>, AfterCExpect, _>();
        fn assert_d<R: Select<Send<RoleD, D1>, I>, I>() {}
        assert_d::<AfterCExpect, _>();
        assert_eq!(
            describe_after::<Rem, Send<RoleC, C1>, _>(),
            format!("Send<{}, {}> => StateC", std::any::type_name::<RoleD>(), std::any::type_name::<D1>())
        );
    }

    define_role_tag!(pub RoleE);
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct E1;
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Kick;

    make_states!(pub Live3 { Start; StateX, StateY, StateZ });
    on_receive!(Start as StartIn {
        Kick => {
            Send<RoleA, A1> => StateX
            | Send<RoleC, C1> => StateY
            | Send<RoleE, E1> => StateZ
        }
    });

    type Rem3 = <Start as OnReceive<Kick>>::Then;
    type AfterEExpect = Cons<Then<Nil, StateZ>, Nil>;

    #[test]
    fn three_way_choice_picks_the_last_alternative() {
        assert_after::<Rem3, Send<RoleE, E1>, AfterEExpect, _>();
        fn assert_done<R: CanFinish<StateZ, crate::typestate::list::Here>>() {}
        assert_done::<AfterEExpect>();
        assert_eq!(describe_after::<Rem3, Send<RoleE, E1>, _>(), "=> StateZ");
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

#[allow(dead_code)]
mod messages_macro {
    use crate::typestate::prelude::*;

    define_messages! {
        /// Outer docs stay on the enum, not the structs.
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
        pub enum Mail {
            Ping { n: u8 },
            #[derive(Copy)]
            Pong(u16),
            Bye,
        }
    }

    #[test]
    fn from_wraps_named_tuple_and_unit() {
        let ping: Mail = Ping { n: 1 }.into();
        assert_eq!(ping, Mail::Ping(Ping { n: 1 }));
        let pong: Mail = Pong(2).into();
        assert_eq!(pong, Mail::Pong(Pong(2)));
        let bye: Mail = Bye.into();
        assert_eq!(bye, Mail::Bye(Bye));
    }

    #[test]
    fn from_mailbox_round_trips_and_rejects() {
        let mail: Mail = Ping { n: 7 }.into();
        assert_eq!(<Ping as FromMailbox<Mail>>::from_mailbox(mail.clone()), Ok(Ping { n: 7 }));
        assert!(matches!(<Bye as FromMailbox<Mail>>::from_mailbox(mail), Err(Mail::Ping(_))));
    }

    #[test]
    fn extra_variant_derive_is_copy() {
        let p = Pong(3);
        let q = p;
        assert_eq!(p, q);
    }

    #[test]
    fn extra_enum_derive_is_ord() {
        assert!(Mail::from(Ping { n: 1 }) < Mail::from(Ping { n: 2 }));
    }
}
