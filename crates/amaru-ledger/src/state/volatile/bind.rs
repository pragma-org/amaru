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

use crate::state::volatile::Resettable;

/// An empty struct to indicate unused left or right delegations in a `Bind`.
#[derive(Debug, Clone)]
pub struct Empty;

/// A structure that captures:
///
/// 1. An optional value `V`
/// 2. An optional left-delegation `L`
/// 3. An optional right-delegation `R`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bind<L, R, V> {
    /// Pending update for the left binding.
    pub left: Resettable<L>,
    /// Pending update for the right binding.
    pub right: Resettable<R>,
    /// Optional value carried by this binding.
    pub value: Option<V>,
}

impl<L, R, V> Default for Bind<L, R, V> {
    fn default() -> Self {
        Self { left: Resettable::default(), right: Resettable::default(), value: None }
    }
}

impl<L, R, V> Bind<L, R, V> {
    /// Borrow all values inside an owned bind.
    pub fn as_refs(&self) -> Bind<&L, &R, &V> {
        Bind { left: self.left.as_refs(), right: self.right.as_refs(), value: self.value.as_ref() }
    }

    /// Absorb a more recent update in place.
    /// A `Set`/`Reset` overrides, `Unchanged` keeps what's here, and a `value: Some(...)` supersedes wholesale.
    pub fn then(&mut self, newer: Self) {
        if newer.value.is_some() {
            *self = newer;
        } else {
            if !matches!(newer.left, Resettable::Unchanged) {
                self.left = newer.left;
            }
            if !matches!(newer.right, Resettable::Unchanged) {
                self.right = newer.right;
            }
        }
    }

    /// Like `Self::then`, but consuming self returning a value instead of mutating in place.
    pub fn and_then(mut self, newer: Self) -> Self {
        self.then(newer);
        self
    }

    /// General clone operation for a Bind.
    pub fn to_owned(&self) -> Self
    where
        L: ToOwned<Owned = L>,
        R: ToOwned<Owned = R>,
        V: ToOwned<Owned = V>,
    {
        Self {
            left: self.left.to_owned(),
            right: self.right.to_owned(),
            value: self.value.as_ref().map(|v| v.to_owned()),
        }
    }
}

impl<L, R, V> Bind<&L, &R, &V> {
    /// Instantiate references within a `Bind` back to owned values.
    pub fn owned(&self) -> Bind<L, R, V>
    where
        L: ToOwned<Owned = L>,
        R: ToOwned<Owned = R>,
        V: ToOwned<Owned = V>,
    {
        Bind { left: self.left.owned(), right: self.right.owned(), value: self.value.map(|v| v.to_owned()) }
    }
}
