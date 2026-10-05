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

//! Destination of a protocol send or call, split so the remainder need not name the mailbox.
//!
//! [`RoleTag`] is the phantom in [`Send<Tag, T>`](super::Send) / [`SendAny<Tag>`](super::SendAny)
//! / [`Call<Tag, T>`](super::Call). [`Role<Tag>`] is a [`StageRef`] wrapper;
//! `Mailbox: From<T>` at the send site. [`IntoRoleCall`] injects the reply slot.
//! Receive has no role (one mailbox, uniquely named cases).

use std::time::Duration;

use serde::de::DeserializeOwned;

use crate::{SendData, StageRef};

/// Name of a send destination, used as the first parameter of [`Send`](super::Send).
///
/// The value passed to [`SessionOps::send`](super::SessionOps::send) is a [`Role`]
/// wrapper that claims this tag and holds the [`StageRef`].
pub trait RoleTag {
    const NAME: &'static str;
}

/// A [`StageRef`] wrapper that may be used wherever [`Send<Tag, T>`](super::Send)
/// appears. Requires [`IntoRoleMail`] at the call site (default: `Mailbox: From<T>`).
pub trait Role<Tag: RoleTag> {
    type Mailbox: SendData;

    fn mailbox(&self) -> &StageRef<Self::Mailbox>;
}

/// Convert a remainder payload into the role mailbox.
///
/// The blanket impl covers `Mailbox: From<T>`. A role that holds extra context
/// (for example a mux protocol id) implements this for payloads that cannot
/// convert by [`From`] alone.
pub trait IntoRoleMail<Tag: RoleTag, T>: Role<Tag> {
    fn encode(&self, msg: T) -> Self::Mailbox;
}

impl<Tag: RoleTag, T, R: Role<Tag>> IntoRoleMail<Tag, T> for R
where
    R::Mailbox: From<T>,
{
    fn encode(&self, msg: T) -> Self::Mailbox {
        let _ = self;
        From::from(msg)
    }
}

/// Convert a remainder payload into a call request, injecting the reply slot.
///
/// Unlike [`IntoRoleMail`], there is no blanket [`From`] impl: the mailbox
/// message must carry [`StageRef<Reply>`] so the callee can answer.
///
/// [`into_call`](Self::into_call) returns the deadline and a closure that only
/// attaches the reply slot. Serialise the payload there, once. The deadline is
/// the length of those bytes, and the closure sends that same buffer.
pub trait IntoRoleCall<Tag: RoleTag, T>: Role<Tag> {
    type Reply: SendData + DeserializeOwned;
    /// Deadline for a payload this role does not size itself.
    const TIMEOUT: Duration;
    fn into_call(self, msg: T) -> (Duration, impl FnOnce(StageRef<Self::Reply>) -> Self::Mailbox + Send + 'static)
    where
        Self: 'static,
        Tag: 'static,
        T: 'static;
}
