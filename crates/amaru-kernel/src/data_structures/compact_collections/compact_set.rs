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

use std::{borrow::Borrow, collections::BTreeSet, iter::FusedIterator, slice};

use super::small_buffer::SmallBuffer;

#[derive(Debug, Clone)]
enum SetStorage<T> {
    Small(SmallBuffer<T>),
    Tree(BTreeSet<T>),
}

/// A sorted set which stores up to `N` values in a single flat allocation before permanently
/// promoting to a [`BTreeSet`].
#[derive(Debug, Clone)]
pub struct CompactSet<T, const N: usize> {
    storage: SetStorage<T>,
}

/// Content-based equality: a promoted set equals a small one holding the same values.
impl<T: Ord, const N: usize> PartialEq for CompactSet<T, N> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().zip(other.iter()).all(|(a, b)| a == b)
    }
}

impl<T: Ord, const N: usize> Eq for CompactSet<T, N> {}

impl<T: Ord, const N: usize> CompactSet<T, N> {
    pub fn new() -> Self {
        Self { storage: SetStorage::Small(SmallBuffer::new()) }
    }

    pub fn len(&self) -> usize {
        match &self.storage {
            SetStorage::Small(entries) => entries.len(),
            SetStorage::Tree(entries) => entries.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn contains<Q>(&self, value: &Q) -> bool
    where
        T: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        match &self.storage {
            SetStorage::Small(entries) => entries.binary_search_by(|candidate| candidate.borrow().cmp(value)).is_ok(),
            SetStorage::Tree(entries) => entries.contains(value),
        }
    }

    pub fn insert(&mut self, value: T) -> bool {
        let small_search = match &self.storage {
            SetStorage::Small(entries) => Some(entries.binary_search_by(|candidate| candidate.cmp(&value))),
            SetStorage::Tree(_) => None,
        };

        match small_search {
            Some(Ok(_)) => false,
            Some(Err(index)) => {
                if self.len() >= N {
                    self.promote();
                }
                match &mut self.storage {
                    SetStorage::Small(entries) => {
                        entries.insert(index, value);
                        true
                    }
                    SetStorage::Tree(entries) => entries.insert(value),
                }
            }
            None => match &mut self.storage {
                SetStorage::Tree(entries) => entries.insert(value),
                SetStorage::Small(_) => unreachable!("storage cannot change between search and borrow"),
            },
        }
    }

    /// Move all small values into the tree, permanently. No-op when already tree-backed.
    fn promote(&mut self) {
        if let SetStorage::Small(entries) = &mut self.storage {
            self.storage = SetStorage::Tree(entries.take().into_iter().collect());
        }
    }

    pub fn remove<Q>(&mut self, value: &Q) -> bool
    where
        T: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        match &mut self.storage {
            SetStorage::Small(entries) => entries
                .binary_search_by(|candidate| candidate.borrow().cmp(value))
                .ok()
                .and_then(|index| entries.remove(index))
                .is_some(),
            SetStorage::Tree(entries) => entries.remove(value),
        }
    }

    pub fn iter(&self) -> CompactSetIter<'_, T> {
        let inner = match &self.storage {
            SetStorage::Small(entries) => CompactSetIterInner::Small(entries.iter()),
            SetStorage::Tree(entries) => CompactSetIterInner::Tree(entries.iter()),
        };
        CompactSetIter { inner }
    }
}

impl<T: Ord, const N: usize> Default for CompactSet<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Ord, const N: usize> FromIterator<T> for CompactSet<T, N> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let iter = iter.into_iter();
        let (lower, _) = iter.size_hint();
        // NOTE: Duplicate handling
        //
        // The lower bound counts duplicates, so a duplicate-heavy iterator with at most N
        // distinct values lands (permanently) in the tree. Call sites collect from other
        // maps/sets, whose elements are already unique, so lower > N means the result
        // outgrows the small regime anyway and we skip the promotion rebuild.
        if lower > N {
            Self { storage: SetStorage::Tree(iter.collect()) }
        } else {
            let mut set = Self { storage: SetStorage::Small(SmallBuffer::with_capacity(lower)) };
            for value in iter {
                set.insert(value);
            }
            set
        }
    }
}

enum CompactSetIntoIterInner<T> {
    Small(std::vec::IntoIter<T>),
    Tree(std::collections::btree_set::IntoIter<T>),
}

pub struct CompactSetIntoIter<T> {
    inner: CompactSetIntoIterInner<T>,
}

impl<T> Iterator for CompactSetIntoIter<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.inner {
            CompactSetIntoIterInner::Small(entries) => entries.next(),
            CompactSetIntoIterInner::Tree(entries) => entries.next(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match &self.inner {
            CompactSetIntoIterInner::Small(entries) => entries.size_hint(),
            CompactSetIntoIterInner::Tree(entries) => entries.size_hint(),
        }
    }
}

impl<T> ExactSizeIterator for CompactSetIntoIter<T> {}
impl<T> FusedIterator for CompactSetIntoIter<T> {}

impl<T: Ord, const N: usize> IntoIterator for CompactSet<T, N> {
    type Item = T;
    type IntoIter = CompactSetIntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        let inner = match self.storage {
            SetStorage::Small(entries) => CompactSetIntoIterInner::Small(entries.into_vec().into_iter()),
            SetStorage::Tree(entries) => CompactSetIntoIterInner::Tree(entries.into_iter()),
        };
        CompactSetIntoIter { inner }
    }
}

enum CompactSetIterInner<'a, T> {
    Small(slice::Iter<'a, T>),
    Tree(std::collections::btree_set::Iter<'a, T>),
}

pub struct CompactSetIter<'a, T> {
    inner: CompactSetIterInner<'a, T>,
}

impl<'a, T> Iterator for CompactSetIter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.inner {
            CompactSetIterInner::Small(entries) => entries.next(),
            CompactSetIterInner::Tree(entries) => entries.next(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match &self.inner {
            CompactSetIterInner::Small(entries) => entries.size_hint(),
            CompactSetIterInner::Tree(entries) => entries.size_hint(),
        }
    }
}

impl<T> ExactSizeIterator for CompactSetIter<'_, T> {}
impl<T> FusedIterator for CompactSetIter<'_, T> {}

impl<'a, T: Ord, const N: usize> IntoIterator for &'a CompactSet<T, N> {
    type Item = &'a T;
    type IntoIter = CompactSetIter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use proptest::{collection, prelude::*};

    use super::CompactSet;

    const SMALL_CAPACITY: usize = 4;

    #[derive(Clone, Debug)]
    enum SetOperation {
        Insert(u8),
        Remove(u8),
        Contains(u8),
    }

    fn set_operation() -> impl Strategy<Value = SetOperation> {
        prop_oneof![
            (0u8..12).prop_map(SetOperation::Insert),
            (0u8..12).prop_map(SetOperation::Remove),
            (0u8..12).prop_map(SetOperation::Contains),
        ]
    }

    proptest! {
        #[test]
        fn compact_set_matches_btree_set(
            initial in collection::vec(0u8..12, 0..24),
            operations in collection::vec(set_operation(), 0..100),
        ) {
            let mut actual = initial.iter().copied().collect::<CompactSet<u8, SMALL_CAPACITY>>();
            let mut model = initial.into_iter().collect::<BTreeSet<_>>();

            prop_assert_eq!(actual.iter().copied().collect::<Vec<_>>(), model.iter().copied().collect::<Vec<_>>());

            for operation in operations {
                match operation {
                    SetOperation::Insert(value) => {
                        prop_assert_eq!(actual.insert(value), model.insert(value));
                    }
                    SetOperation::Remove(value) => {
                        prop_assert_eq!(actual.remove(&value), model.remove(&value));
                    }
                    SetOperation::Contains(value) => {
                        prop_assert_eq!(actual.contains(&value), model.contains(&value));
                    }
                }

                prop_assert_eq!(actual.len(), model.len());
                prop_assert_eq!(actual.is_empty(), model.is_empty());
                prop_assert_eq!(actual.iter().copied().collect::<Vec<_>>(), model.iter().copied().collect::<Vec<_>>());
            }

            prop_assert_eq!((&actual).into_iter().copied().collect::<Vec<_>>(), model.iter().copied().collect::<Vec<_>>());
            prop_assert_eq!(actual.clone().into_iter().collect::<Vec<_>>(), model.clone().into_iter().collect::<Vec<_>>());
        }
    }
}
