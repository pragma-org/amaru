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
    Exists(T),
    Gone,
    Unknown,
}

impl<T: Copy> Existence<&T> {
    pub fn copied(self) -> Existence<T> {
        match self {
            Self::Exists(t) => Existence::Exists(*t),
            Self::Gone => Existence::Gone,
            Self::Unknown => Existence::Unknown,
        }
    }
}

impl<L: ToOwned<Owned = L>, R: ToOwned<Owned = R>, V: ToOwned<Owned = V>> Existence<Bind<&L, &R, &V>> {
    pub fn to_owned(self) -> Existence<Bind<L, R, V>> {
        match self {
            Self::Exists(bind) => Existence::Exists(bind.to_owned()),
            Self::Gone => Existence::Gone,
            Self::Unknown => Existence::Unknown,
        }
    }
}

impl<T> Existence<T> {
    /// Layer this verdict over an `older` one, evaluated lazily
    pub fn or_else(self, older: impl FnOnce() -> Self) -> Self {
        match self {
            Self::Exists(..) | Self::Gone => self,
            Self::Unknown => older(),
        }
    }
}

impl<L, R, V> Existence<Bind<L, R, V>> {
    /// Layer this verdict over an `older` one, evaluated lazily
    pub fn or_else_bind(self, older: impl FnOnce() -> Self) -> Self {
        match self {
            Existence::Unknown => older(),

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
            Existence::Exists(newer)
                if newer.value.is_none()
                    && let Existence::Exists(mut older) = older() =>
            {
                older.then(newer);
                Existence::Exists(older)
            }

            Existence::Gone | Existence::Exists(..) => self,
        }
    }
}
