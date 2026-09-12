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

use std::{cmp::Ordering, ops::Deref};

/// A vector kept sorted for the small regime of the compact collections.
///
/// Reads go through the [`Deref`] to a slice. Growth is exact rather than amortized.
#[derive(Debug, Clone)]
pub(super) struct SmallSortedBuffer<T> {
    entries: Vec<T>,
}

impl<T> SmallSortedBuffer<T> {
    pub(super) fn new() -> Self {
        Self { entries: Vec::new() }
    }

    pub(super) fn with_capacity(capacity: usize) -> Self {
        Self { entries: Vec::with_capacity(capacity) }
    }

    /// The entry matching `probe`, where `probe` follows the buffer's sort order.
    pub(super) fn get_by(&self, probe: impl FnMut(&T) -> Ordering) -> Option<&T> {
        self.entries.binary_search_by(probe).ok().map(|index| &self.entries[index])
    }

    /// Remove and return the entry matching `probe`, where `probe` follows the buffer's sort order.
    pub(super) fn remove_by(&mut self, probe: impl FnMut(&T) -> Ordering) -> Option<T> {
        self.entries.binary_search_by(probe).ok().map(|index| self.entries.remove(index))
    }

    /// Insert `value` at `index`, which must be its insertion point in sort order.
    pub(super) fn insert_at(&mut self, index: usize, value: T) -> &mut T {
        self.entries.reserve_exact(1);
        self.entries.insert(index, value);
        &mut self.entries[index]
    }

    /// Remove and return the entry at `index`.
    pub(super) fn remove_at(&mut self, index: usize) -> T {
        self.entries.remove(index)
    }

    /// Mutable access to the entry at `index`; the caller must not change how the entry sorts
    /// relative to its neighbours.
    pub(super) fn get_at_mut(&mut self, index: usize) -> &mut T {
        &mut self.entries[index]
    }

    pub(super) fn take(&mut self) -> Vec<T> {
        std::mem::take(&mut self.entries)
    }

    pub(super) fn into_vec(self) -> Vec<T> {
        self.entries
    }
}

impl<T> Deref for SmallSortedBuffer<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}
