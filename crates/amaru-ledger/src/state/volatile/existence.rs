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

use crate::state::volatile::Bind;

/// A volatile layer's verdict on an entity.
/// - `T` is the resolved record.
/// - `Gone` is a tombstone, so don't fall back to the stable store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Existence<T> {
    /// The volatile layer resolves the entity to a concrete value.
    Exists(T),
    /// The volatile layer knows the entity has been removed.
    Gone,
    /// The volatile layer has no conclusive information and may need historical state.
    Unknown,
}

impl<T: Copy> Existence<&T> {
    /// Copy a borrowed payload out of an existence verdict.
    pub fn copied(self) -> Existence<T> {
        match self {
            Self::Exists(t) => Existence::Exists(*t),
            Self::Gone => Existence::Gone,
            Self::Unknown => Existence::Unknown,
        }
    }
}

impl<T> Existence<T> {
    /// Borrow the payload carried by an existence verdict.
    pub fn as_ref(&self) -> Existence<&T> {
        match self {
            Self::Exists(v) => Existence::Exists(v),
            Self::Gone => Existence::Gone,
            Self::Unknown => Existence::Unknown,
        }
    }
}

impl<L, R, V> Existence<Bind<L, R, V>> {
    /// Borrow the payload carried by an existence verdict.
    pub fn as_refs(&self) -> Existence<Bind<&L, &R, &V>> {
        match self {
            Self::Exists(bind) => Existence::Exists(bind.as_refs()),
            Self::Gone => Existence::Gone,
            Self::Unknown => Existence::Unknown,
        }
    }

    /// Return this verdict when it is conclusive, otherwise lazily fall back to an older one.
    pub fn or_else(self, older: impl FnOnce() -> Self) -> Self {
        Self::fold(std::iter::once(self).chain(std::iter::once_with(older)))
    }

    /// Fold a sequence of existence entries, from the most recent to least recent. Short-circuit as
    /// soon as a conclusive result is reached.
    pub fn fold(mut iterator: impl Iterator<Item = Self>) -> Self {
        let mut fold: Self = Self::Unknown;
        loop {
            return match fold {
                gone @ Self::Gone => gone,

                // If this is rebinding (i.e. value is None), then we must still take into
                // account the previous value, if any.
                //
                // NOTE: superfluous 'is_none()' check actually not superfluous
                //
                // The `value.is_none()` guard may seem redundant with the implementation of `then`.
                // But it allows to only lazily get the `older` state when we truly have to. Indeed, if
                // there already exists a newer value, that means the object was entirely re-recreated
                // and there's no need to fetch the previous state for any left or right binds. It's
                // just been overidden.
                //
                // Hence, the guard doesn't fundamentally changes the logic since `older.then(newer)`
                // would simply override `older` with `newer` when the value exists; but it saves us
                // from fetching the `older` to begin with.
                Self::Exists(newer) => {
                    if newer.value.is_none()
                        && let Some(Self::Exists(mut older)) = iterator.next()
                    {
                        older.then(newer);
                        fold = Self::Exists(older);
                        continue;
                    }

                    Self::Exists(newer)
                }

                unknown @ Self::Unknown => {
                    if let Some(older) = iterator.next() {
                        fold = older;
                        continue;
                    }

                    unknown
                }
            };
        }
    }
}

impl<L, R, V> Existence<Bind<&L, &R, &V>> {
    /// Materialize a borrowed existence verdict back into an owned one.
    pub fn owned(&self) -> Existence<Bind<L, R, V>>
    where
        L: ToOwned<Owned = L>,
        R: ToOwned<Owned = R>,
        V: ToOwned<Owned = V>,
    {
        match self {
            Self::Exists(bind) => Existence::Exists(bind.owned()),
            Self::Gone => Existence::Gone,
            Self::Unknown => Existence::Unknown,
        }
    }
}
