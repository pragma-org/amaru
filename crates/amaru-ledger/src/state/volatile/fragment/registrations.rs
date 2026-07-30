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

use std::{iter, ops::Deref};

/// A compact representation of the registrations observed for a key within a single epoch-local
/// diff.
///
/// A key can be registered at most once and then re-registered once more in the same reduced
/// state, so this stores the current registration together with an optional replacement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registrations<V>((V, Option<V>));

impl<V> Registrations<V> {
    /// Create a registrations set containing a single current registration.
    pub fn new(v: V) -> Self {
        Self((v, None))
    }

    /// Consume the structure and return the current registration together with its optional
    /// re-registration.
    pub fn into_inner(self) -> (V, Option<V>) {
        self.0
    }

    /// Record a more recent registration, replacing any previous re-registration.
    pub fn next(&mut self, v: V) {
        let inner = &mut self.0;
        inner.1 = Some(v);
    }

    /// Return the most recent registration, falling back to the original one if no
    /// re-registration exists.
    pub fn last(&self) -> &V {
        let inner = &self.0;
        inner.1.as_ref().unwrap_or(&inner.0)
    }

    /// Consume the structure and return only its most recent registration.
    pub fn into_last(self) -> V {
        let inner = self.0;
        inner.1.unwrap_or(inner.0)
    }

    /// Borrow all registrations stored in this structure.
    pub fn as_refs(&self) -> Registrations<&V> {
        Registrations((&self.0.0, self.0.1.as_ref()))
    }

    /// Borrow all registrations by dereferencing their payloads.
    pub fn as_derefs<T>(&self) -> Registrations<&T>
    where
        V: Deref<Target = T>,
    {
        Registrations((self.0.0.deref(), self.0.1.as_deref()))
    }

    /// Iterate over the current registration and, if present, its re-registration.
    pub fn iter(&self) -> impl Iterator<Item = &V> {
        iter::once(&self.0.0).chain(self.0.1.iter())
    }
}

impl<V> IntoIterator for Registrations<V> {
    type Item = V;
    type IntoIter = iter::Chain<iter::Once<V>, std::option::IntoIter<V>>;

    fn into_iter(self) -> Self::IntoIter {
        let (current, next) = self.0;
        iter::once(current).chain(next)
    }
}
