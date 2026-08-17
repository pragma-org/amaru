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

//! Opt-in session-typed layer over [`Effects`](crate::Effects).
//!
//! Pure-stage transitions are invoked *because* a message arrived; there is no
//! `Effects::receive`. This module treats that implicit receive as the constructor:
//! from a protocol state value (usually taken out of a [`make_states`](crate::make_states)
//! enum), [`State::receive`] consumes the receive allowance for a particular
//! input variant and returns a [`Session`] whose remaining effects are still
//! allowed. [`Session::finish`] is the only way to obtain the next state; wrap
//! it with [`Into`] into the live enum so the programmer cannot pick the next
//! case by hand. [`State::convert_input`] turns the mailbox message into that
//! state's input enum (`Ok`) or returns it unmatched (`Err`).
//!
//! [`Send<Tag, T>`](Send) names a destination [role tag](RoleTag). The value
//! passed to [`Session::send`] is a [`Role`] wrapper around a
//! [`StageRef`](crate::StageRef) claiming that tag; sending `T` requires
//! `Mailbox: From<T>`. Receive needs no role: a stage has a single mailbox of
//! uniquely named cases.
//!
//! Existing stages keep using [`Effects`](crate::Effects) unchanged.

mod effect;
mod list;
mod macros;
mod role;
mod session;

pub use effect::{
    AddStage, Call, CancelSchedule, Clock, Effect, External, Receive, Repeat, Schedule, Send, Terminate, Wait,
};
pub use list::{Clean, Cons, FmtPar, Here, Nil, Select, There};
pub use role::{Role, RoleTag};
pub use session::{
    ExtractInput, FromMailbox, InitialState, Marker, NotInitialState, OnReceive, Session, State, To, describe_receive,
    initial_state,
};

pub mod prelude {
    pub use super::{
        AddStage, Call, CancelSchedule, Clock, Cons, External, ExtractInput, FromMailbox, Nil, OnReceive, Receive,
        Repeat, Role, RoleTag, Schedule, Send, Session, State, Terminate, To, Wait, initial_state,
    };
    pub use crate::{define_mailbox, define_role, define_role_tag, make_states, on_receive};
}

#[cfg(test)]
mod tests;
