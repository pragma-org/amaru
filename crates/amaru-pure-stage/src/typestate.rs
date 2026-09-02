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

//! Opt-in session types over [`Effects`](crate::Effects).
//!
//! Receive is implicit (the stage was invoked). [`State::receive`] consumes that
//! allowance and returns a [`Session`] whose type is the remainder. [`Session::finish`]
//! is the only constructor of a non-initial state; put it in the live enum via [`Into`].
//!
//! Remainder syntax: `,` sequences effects; `|` before `=> S` is parallel (all
//! branches); `|` between `=> S` groups is exclusive choice of next state
//! (any number of alternatives). [`Repeat<E>`](Repeat) is a Kleene star: a
//! single effect, or a sequence via [`star`](crate::star). Selecting the first
//! step unrolls the rest in front of the same `Repeat`. Selecting a later
//! step discards the star (zero iterations).
//!
//! **Limits:** parallel and choice are flat `Cons` lists (not tree-associative).
//! Sequences are ordered. When several parallel heads match, the **leftmost**
//! wins. Two choice alternatives with the same head are ambiguous (payload
//! inference would otherwise stick to the first alternative). `finish` strips
//! `Repeat` only at each branch prefix. [`SetTimeout`] / [`ClearTimeout`] are
//! required remainders for agency timers (`Effects::set_timeout`). Existing
//! stages keep using [`Effects`](crate::Effects).

mod effect;
mod list;
mod macros;
mod role;
mod session;

pub use effect::{
    AddStage, Call, CancelSchedule, ClearTimeout, Clock, Effect, External, Receive, Repeat, Schedule, Send, SendAny,
    SetTimeout, Terminate, Wait,
};
pub use list::{CanFinish, Clean, Cons, FmtPar, Here, Nil, Select, Then};
pub use role::{Role, RoleTag};
pub use session::{
    ExtractInput, FromMailbox, InitialState, Marker, NotInitialState, OnReceive, Session, State, To, initial_state,
};

pub mod prelude {
    pub use super::{
        AddStage, Call, CancelSchedule, ClearTimeout, Clock, Cons, External, ExtractInput, FromMailbox, Nil, OnReceive,
        Receive, Repeat, Role, RoleTag, Schedule, Send, SendAny, Session, SetTimeout, State, Terminate, To, Wait,
        initial_state,
    };
    pub use crate::{define_mailbox, define_role, define_role_tag, make_states, on_receive, star};
}

#[cfg(test)]
mod tests;
