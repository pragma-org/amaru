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

//! Destination of a protocol send, split so the remainder need not name the mailbox.
//!
//! [`RoleTag`] is the phantom in [`Send<Tag, T>`](super::Send) / [`SendAny<Tag>`](super::SendAny).
//! [`Role<Tag>`] is a [`StageRef`] wrapper; `Mailbox: From<T>` at the call site.
//! Receive has no role (one mailbox, uniquely named cases).

use crate::{SendData, StageRef};

/// Name of a send destination, used as the first parameter of [`Send`](super::Send).
///
/// The value passed to [`Session::send`](super::Session::send) is a [`Role`]
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
