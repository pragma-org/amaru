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

//! Shared protocol types for UI snippets. Avoids `make_states!` / `define_role!`
//! so rustc does not need `serde` as a crate-level `--extern`.

use amaru_pure_stage::{
    StageRef,
    typestate::{InitialState, Marker, NotInitialState, Role, RoleTag, State},
};

pub struct Idle(Marker);
impl State for Idle {
    const NAME: &'static str = "Idle";
    fn make(marker: Marker) -> Self {
        Idle(marker)
    }
    type Initial = InitialState;
}

pub struct Done(Marker);
impl State for Done {
    const NAME: &'static str = "Done";
    fn make(marker: Marker) -> Self {
        Done(marker)
    }
    type Initial = NotInitialState;
}

pub struct ToPeer;
impl RoleTag for ToPeer {
    const NAME: &'static str = "ToPeer";
}

pub struct Peer(StageRef<String>);
impl Role<ToPeer> for Peer {
    type Mailbox = String;
    fn mailbox(&self) -> &StageRef<String> {
        &self.0
    }
}
