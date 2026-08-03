// Copyright 2024 PRAGMA
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

use std::mem;

/// A pending update to an optional field.
///
/// This distinguishes between setting a new value, clearing the field, and leaving it untouched.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Resettable<A> {
    /// Replace the field with the contained value.
    Set(A),
    /// Clear the field.
    Reset,
    /// Leave the field as it was.
    #[default]
    Unchanged,
}

impl<A> From<Option<A>> for Resettable<A> {
    fn from(opt: Option<A>) -> Self {
        match opt {
            None => Resettable::Reset,
            Some(r) => Resettable::Set(r),
        }
    }
}

impl<A> Resettable<A> {
    /// Map a function onto the value if any.
    pub fn map<B>(self, to: impl FnOnce(A) -> B) -> Resettable<B> {
        match self {
            Self::Set(a) => Resettable::Set(to(a)),
            Self::Reset => Resettable::Reset,
            Self::Unchanged => Resettable::Unchanged,
        }
    }

    /// Apply this change to `value`, returning the previous content when a change occurred.
    ///
    /// - `Unchanged` => returns `None` and leaves `value` as-is
    /// - `Set(new)`  => replaces `value` with `Some(new)` and returns the old `Option<A>`
    /// - `Reset`     => sets `value` to `None` and returns the old `Option<A>`
    pub fn set_or_reset(self, value: &mut Option<A>) -> Option<A> {
        match self {
            Self::Set(new) => Option::replace(value, new),
            Self::Reset => mem::take(value),
            Self::Unchanged => None,
        }
    }

    /// Borrow the contained value while preserving the reset semantics.
    pub fn as_refs(&self) -> Resettable<&A> {
        match self {
            Self::Set(value) => Resettable::Set(value),
            Self::Reset => Resettable::Reset,
            Self::Unchanged => Resettable::Unchanged,
        }
    }

    /// Transform into an `Option`, using the default value `when_unchanged` for the `Unchanged`
    /// case.
    pub fn into_option(self, when_unchanged: Option<A>) -> Option<A> {
        match self {
            Self::Set(value) => Some(value),
            Self::Reset => None,
            Self::Unchanged => when_unchanged,
        }
    }

    /// Materialize a borrowed reset instruction back into an owned one.
    pub fn to_owned(&self) -> Self
    where
        A: ToOwned<Owned = A>,
    {
        match self {
            Self::Set(a) => Self::Set((*a).to_owned()),
            Self::Reset => Self::Reset,
            Self::Unchanged => Self::Unchanged,
        }
    }
}

impl<A> Resettable<&A> {
    /// Materialize a borrowed reset instruction back into an owned one.
    pub fn owned(&self) -> Resettable<A>
    where
        A: ToOwned<Owned = A>,
    {
        match self {
            Self::Set(a) => Resettable::Set((*a).to_owned()),
            Self::Reset => Resettable::Reset,
            Self::Unchanged => Resettable::Unchanged,
        }
    }

    /// Transform into an `Option`, using the default value `when_unchanged` for the `Unchanged`
    /// case.
    pub fn to_option(&self, when_unchanged: Option<&A>) -> Option<A>
    where
        A: ToOwned<Owned = A>,
    {
        match self {
            Self::Set(value) => Some((*value).to_owned()),
            Self::Reset => None,
            Self::Unchanged => when_unchanged.map(|value| value.to_owned()),
        }
    }
}
