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
//! allowance and returns a [`Session`] whose type is the remainder. [`SessionOps::finish`]
//! is the only constructor of a non-initial state; put it in the live enum via [`Into`].
//!
//! Remainder syntax: `,` sequences effects; `|` before `=> S` is parallel (all
//! branches); `|` between `=> S` groups is exclusive choice of next state
//! (any number of alternatives). [`Repeat<E>`](Repeat) is a Kleene star: a
//! single effect, or a sequence via [`star`](crate::star). Selecting the first
//! step unrolls the rest in front of the same `Repeat`. Selecting a later
//! step discards the star (zero iterations) when the selected type is given
//! in full (type-level [`Select`]); [`SessionOps::send`]
//! / [`SessionOps::call`] cannot skip that way because
//! `Repeat<Send<Role, T>>` / `Repeat<Call<Role, T>>` unifies `T` with the
//! star. Use [`SessionOps::discard_repeat`]
//! after the last iteration.
//!
//! Hover a failing `send` still shows the encoding. Dump the surface syntax
//! (`"Send<Role, T> => Idle"`) with [`reveal_remainder`](crate::reveal_remainder)
//! (`reveal_remainder!(streaming)` → E0080 with that string). [`Session::remainder`](session::Session::remainder)
//! returns the same `&str` at runtime.
//!
//! **Limits:** sequences, parallel branches, and choice alternatives are tuples
//! of length at most 10 ([`Choice`] / [`Par`] / [`Repeat`] wrap those tuples).
//! Sequences are ordered. When several parallel heads match, the **leftmost**
//! wins. Two choice alternatives with the same head are ambiguous (payload
//! inference would otherwise stick to the first alternative). `finish` strips
//! `Repeat` only at each branch prefix. A `Repeat` that is a whole parallel
//! branch (`Repeat<Terminate> | External<…> => S`) stays selectable while the
//! other branch runs; a `Repeat` sequenced before a suffix is discarded when
//! that suffix is selected (zero iterations). [`Wait`] / [`SetTimeout`] /
//! [`ClearTimeout`] / [`Clock`] / [`External`] / [`Detach`] / [`Schedule`] /
//! [`CancelSchedule`] are selectable remainders (`Effects::wait`,
//! `set_timeout`, `clock`, `external`, `detach`, `schedule_at`,
//! `cancel_schedule`). Existing stages keep using [`Effects`](crate::Effects).

mod describe;
mod effect;
mod list;
mod macros;
mod occupancy;
mod role;
mod session;

pub use describe::{ConstDesc, Remainder, remainder_ctfe_panic};
pub use effect::{
    AddStage, Call, CancelSchedule, ClearTimeout, Clock, Detach, Effect, External, Receive, Repeat, Schedule, Send,
    SendAny, SetTimeout, Terminate, Wait,
};
pub use list::{CanFinish, Choice, Clean, DiscardRepeat, FinishIn, FmtPar, Here, Par, Select, Take, Then};
pub use occupancy::{Occupancy, OccupancyOf};
pub use role::{IntoRoleCall, IntoRoleMail, Role, RoleTag};
pub use session::{
    ExtractInput, FromMailbox, InitialState, Marker, NotInitialState, OnReceive, SendAnyOp, Session, SessionOps, State,
    To, initial_state,
};

pub mod prelude {
    pub use super::{
        AddStage, Call, CancelSchedule, Choice, ClearTimeout, Clock, Detach, External, ExtractInput, FromMailbox,
        IntoRoleCall, IntoRoleMail, Occupancy, OccupancyOf, OnReceive, Par, Receive, Remainder, Repeat, Role, RoleTag,
        Schedule, Send, SendAny, Session, SessionOps, SetTimeout, State, Terminate, To, Wait, initial_state,
    };
    pub use crate::{
        define_mailbox, define_messages, define_role, define_role_tag, make_states, on_receive, reveal_remainder, star,
    };
}

#[cfg(test)]
mod tests;
