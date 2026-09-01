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

use std::ops::Deref;

/// A sorted vector for the small regime.
///
/// Derefs to a slice for reads; mutations go through the inherent methods, which preserve
/// ordering and grow exactly rather than amortized growth.
#[derive(Debug, Clone)]
pub(super) struct SmallBuffer<T> {
    entries: Vec<T>,
}

impl<T> SmallBuffer<T> {
    pub(super) fn new() -> Self {
        Self { entries: Vec::new() }
    }

    pub(super) fn with_capacity(capacity: usize) -> Self {
        Self { entries: Vec::with_capacity(capacity) }
    }

    pub(super) fn insert(&mut self, index: usize, value: T) {
        self.entries.reserve_exact(1);
        self.entries.insert(index, value);
    }

    pub(super) fn remove(&mut self, index: usize) -> Option<T> {
        (index < self.entries.len()).then(|| self.entries.remove(index))
    }

    /// Mutable access to a single entry; the caller must not reorder it relative to its neighbours.
    pub(super) fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        self.entries.get_mut(index)
    }

    pub(super) fn take(&mut self) -> Vec<T> {
        std::mem::take(&mut self.entries)
    }

    pub(super) fn into_vec(self) -> Vec<T> {
        self.entries
    }
}

impl<T> Deref for SmallBuffer<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}
