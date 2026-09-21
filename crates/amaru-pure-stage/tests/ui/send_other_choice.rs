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

use amaru_pure_stage::{
    StageRef,
    typestate::{InitialState, Marker, NotInitialState, Role, RoleTag, State, prelude::*},
};

struct Idle(Marker);
impl State for Idle {
    const NAME: &'static str = "Idle";
    fn make(marker: Marker) -> Self {
        Idle(marker)
    }
    type Initial = InitialState;
}
struct StateA(Marker);
impl State for StateA {
    const NAME: &'static str = "StateA";
    fn make(marker: Marker) -> Self {
        StateA(marker)
    }
    type Initial = NotInitialState;
}
struct StateC(Marker);
impl State for StateC {
    const NAME: &'static str = "StateC";
    fn make(marker: Marker) -> Self {
        StateC(marker)
    }
    type Initial = NotInitialState;
}

macro_rules! tag_role {
    ($tag:ident, $role:ident) => {
        struct $tag;
        impl RoleTag for $tag {
            const NAME: &'static str = stringify!($tag);
        }
        struct $role(StageRef<u8>);
        impl Role<$tag> for $role {
            type Mailbox = u8;
            fn mailbox(&self) -> &StageRef<u8> {
                &self.0
            }
        }
    };
}
tag_role!(RoleA, DestA);
tag_role!(RoleB, DestB);
tag_role!(RoleC, DestC);
tag_role!(RoleD, DestD);

on_receive!(Idle, Go => Send<RoleA, u8>, Send<RoleB, u8> => StateA | Send<RoleC, u8>, Send<RoleD, u8> => StateC);
struct Go;

async fn go<M: core::marker::Send>(s: Idle, a: &DestA, d: &DestD, eff: amaru_pure_stage::Effects<M>) {
    let s = s.receive(Go, eff).send(a, 1u8).await;
    let _ = s.send(d, 1u8).await;
}
