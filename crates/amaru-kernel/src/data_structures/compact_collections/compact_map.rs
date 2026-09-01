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

use std::{
    borrow::Borrow,
    collections::{BTreeMap, btree_map},
    iter::FusedIterator,
    slice,
};

use super::small_sorted_buffer::SmallSortedBuffer;

#[derive(Debug, Clone)]
enum MapStorage<K, V> {
    Small(SmallSortedBuffer<(K, V)>),
    Tree(BTreeMap<K, V>),
}

/// A sorted map which stores up to `N` entries in a single flat allocation before permanently
/// promoting to a [`BTreeMap`].
#[derive(Debug, Clone)]
pub struct CompactMap<K, V, const N: usize> {
    storage: MapStorage<K, V>,
}

/// Content-based equality: a promoted map equals a small one holding the same entries.
impl<K: Ord, V: PartialEq, const N: usize> PartialEq for CompactMap<K, V, N> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().zip(other.iter()).all(|(a, b)| a == b)
    }
}

impl<K: Ord, V: Eq, const N: usize> Eq for CompactMap<K, V, N> {}

impl<K: Ord, V, const N: usize> CompactMap<K, V, N> {
    pub fn new() -> Self {
        Self { storage: MapStorage::Small(SmallSortedBuffer::new()) }
    }

    pub fn len(&self) -> usize {
        match &self.storage {
            MapStorage::Small(entries) => entries.len(),
            MapStorage::Tree(entries) => entries.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        match &self.storage {
            MapStorage::Small(entries) => {
                entries.get_by(|(candidate, _)| candidate.borrow().cmp(key)).map(|(_, value)| value)
            }
            MapStorage::Tree(entries) => entries.get(key),
        }
    }

    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.get(key).is_some()
    }

    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        match self.entry(key) {
            Entry::Occupied(mut entry) => Some(entry.insert(value)),
            Entry::Vacant(entry) => {
                entry.insert(value);
                None
            }
        }
    }

    /// A single-lookup view into the entry for `key`. Inserting through
    /// a vacant entry promotes to the tree when the small buffer is full.
    pub fn entry(&mut self, key: K) -> Entry<'_, K, V, N> {
        let small_search = match &self.storage {
            MapStorage::Small(entries) => Some(entries.binary_search_by(|(candidate, _)| candidate.cmp(&key))),
            MapStorage::Tree(_) => None,
        };

        match small_search {
            Some(Ok(index)) => match &mut self.storage {
                MapStorage::Small(entries) => Entry::Occupied(OccupiedEntry(OccupiedInner::Small { entries, index })),
                MapStorage::Tree(_) => unreachable!("storage cannot change between search and borrow"),
            },
            Some(Err(index)) => Entry::Vacant(VacantEntry(VacantInner::Small { map: self, key, index })),
            None => match &mut self.storage {
                MapStorage::Tree(entries) => match entries.entry(key) {
                    btree_map::Entry::Occupied(entry) => Entry::Occupied(OccupiedEntry(OccupiedInner::Tree(entry))),
                    btree_map::Entry::Vacant(entry) => Entry::Vacant(VacantEntry(VacantInner::Tree(entry))),
                },
                MapStorage::Small(_) => unreachable!("storage cannot change between search and borrow"),
            },
        }
    }

    /// Move all small entries into the tree, permanently, and return it. No-op when already
    /// tree-backed.
    fn promote(&mut self) -> &mut BTreeMap<K, V> {
        if let MapStorage::Small(entries) = &mut self.storage {
            self.storage = MapStorage::Tree(entries.take().into_iter().collect());
        }
        match &mut self.storage {
            MapStorage::Tree(entries) => entries,
            MapStorage::Small(_) => unreachable!("promotion always ends in tree storage"),
        }
    }

    pub fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        match &mut self.storage {
            MapStorage::Small(entries) => {
                entries.remove_by(|(candidate, _)| candidate.borrow().cmp(key)).map(|(_, value)| value)
            }
            MapStorage::Tree(entries) => entries.remove(key),
        }
    }

    pub fn iter(&self) -> CompactMapIter<'_, K, V> {
        let inner = match &self.storage {
            MapStorage::Small(entries) => CompactMapIterInner::Small(entries.iter()),
            MapStorage::Tree(entries) => CompactMapIterInner::Tree(entries.iter()),
        };
        CompactMapIter { inner }
    }

    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.iter().map(|(key, _)| key)
    }
}

/// A view into a single entry in a [`CompactMap`], which is either occupied or vacant.
pub enum Entry<'a, K, V, const N: usize> {
    Occupied(OccupiedEntry<'a, K, V>),
    Vacant(VacantEntry<'a, K, V, N>),
}

/// A view into an occupied entry in a [`CompactMap`].
pub struct OccupiedEntry<'a, K, V>(OccupiedInner<'a, K, V>);

enum OccupiedInner<'a, K, V> {
    Small { entries: &'a mut SmallSortedBuffer<(K, V)>, index: usize },
    Tree(btree_map::OccupiedEntry<'a, K, V>),
}

impl<'a, K: Ord, V> OccupiedEntry<'a, K, V> {
    pub fn get(&self) -> &V {
        match &self.0 {
            OccupiedInner::Small { entries, index } => {
                let (_, value) = &entries[*index];
                value
            }
            OccupiedInner::Tree(entry) => entry.get(),
        }
    }

    pub fn get_mut(&mut self) -> &mut V {
        match &mut self.0 {
            OccupiedInner::Small { entries, index } => {
                let (_, value) = entries.get_at_mut(*index);
                value
            }
            OccupiedInner::Tree(entry) => entry.get_mut(),
        }
    }

    /// Convert the entry into a mutable reference bound to the map's lifetime.
    pub fn into_mut(self) -> &'a mut V {
        match self.0 {
            OccupiedInner::Small { entries, index } => {
                let (_, value) = entries.get_at_mut(index);
                value
            }
            OccupiedInner::Tree(entry) => entry.into_mut(),
        }
    }

    /// Replace the value, returning the previous one.
    pub fn insert(&mut self, value: V) -> V {
        std::mem::replace(self.get_mut(), value)
    }

    /// Remove the entry from the map, returning its value.
    pub fn remove(self) -> V {
        match self.0 {
            OccupiedInner::Small { entries, index } => entries.remove_at(index).1,
            OccupiedInner::Tree(entry) => entry.remove(),
        }
    }
}

/// A view into a vacant entry in a [`CompactMap`].
pub struct VacantEntry<'a, K, V, const N: usize>(VacantInner<'a, K, V, N>);

enum VacantInner<'a, K, V, const N: usize> {
    Small { map: &'a mut CompactMap<K, V, N>, key: K, index: usize },
    Tree(btree_map::VacantEntry<'a, K, V>),
}

impl<'a, K: Ord, V, const N: usize> VacantEntry<'a, K, V, N> {
    /// Insert a value, promoting the map to the tree when the small buffer is full.
    pub fn insert(self, value: V) -> &'a mut V {
        match self.0 {
            VacantInner::Small { map, key, index } => {
                if map.len() >= N {
                    return map.promote().entry(key).or_insert(value);
                }

                match &mut map.storage {
                    MapStorage::Small(entries) => {
                        let (_, value) = entries.insert_at(index, (key, value));
                        value
                    }
                    MapStorage::Tree(_) => unreachable!("vacant small entries only exist on small storage"),
                }
            }
            VacantInner::Tree(entry) => entry.insert(value),
        }
    }
}

impl<K: Ord, V, const N: usize> Default for CompactMap<K, V, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Ord, V, const N: usize> FromIterator<(K, V)> for CompactMap<K, V, N> {
    fn from_iter<T: IntoIterator<Item = (K, V)>>(iter: T) -> Self {
        let iter = iter.into_iter();
        let (lower, _) = iter.size_hint();
        // NOTE: Duplicate handling
        //
        // See CompactSet::from_iter: keys are unique at every call site, so lower > N means
        // the result outgrows the small regime and we skip the promotion rebuild.
        if lower > N {
            Self { storage: MapStorage::Tree(iter.collect()) }
        } else {
            let mut map = Self { storage: MapStorage::Small(SmallSortedBuffer::with_capacity(lower)) };
            for (key, value) in iter {
                map.insert(key, value);
            }
            map
        }
    }
}

enum CompactMapIntoIterInner<K, V> {
    Small(std::vec::IntoIter<(K, V)>),
    Tree(btree_map::IntoIter<K, V>),
}

pub struct CompactMapIntoIter<K, V> {
    inner: CompactMapIntoIterInner<K, V>,
}

impl<K, V> Iterator for CompactMapIntoIter<K, V> {
    type Item = (K, V);

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.inner {
            CompactMapIntoIterInner::Small(entries) => entries.next(),
            CompactMapIntoIterInner::Tree(entries) => entries.next(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match &self.inner {
            CompactMapIntoIterInner::Small(entries) => entries.size_hint(),
            CompactMapIntoIterInner::Tree(entries) => entries.size_hint(),
        }
    }
}

impl<K, V> ExactSizeIterator for CompactMapIntoIter<K, V> {}
impl<K, V> FusedIterator for CompactMapIntoIter<K, V> {}

impl<K: Ord, V, const N: usize> IntoIterator for CompactMap<K, V, N> {
    type Item = (K, V);
    type IntoIter = CompactMapIntoIter<K, V>;

    fn into_iter(self) -> Self::IntoIter {
        let inner = match self.storage {
            MapStorage::Small(entries) => CompactMapIntoIterInner::Small(entries.into_vec().into_iter()),
            MapStorage::Tree(entries) => CompactMapIntoIterInner::Tree(entries.into_iter()),
        };
        CompactMapIntoIter { inner }
    }
}

enum CompactMapIterInner<'a, K, V> {
    Small(slice::Iter<'a, (K, V)>),
    Tree(btree_map::Iter<'a, K, V>),
}

pub struct CompactMapIter<'a, K, V> {
    inner: CompactMapIterInner<'a, K, V>,
}

impl<'a, K, V> Iterator for CompactMapIter<'a, K, V> {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.inner {
            CompactMapIterInner::Small(entries) => entries.next().map(|(key, value)| (key, value)),
            CompactMapIterInner::Tree(entries) => entries.next(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match &self.inner {
            CompactMapIterInner::Small(entries) => entries.size_hint(),
            CompactMapIterInner::Tree(entries) => entries.size_hint(),
        }
    }
}

impl<K, V> ExactSizeIterator for CompactMapIter<'_, K, V> {}
impl<K, V> FusedIterator for CompactMapIter<'_, K, V> {}

impl<'a, K: Ord, V, const N: usize> IntoIterator for &'a CompactMap<K, V, N> {
    type Item = (&'a K, &'a V);
    type IntoIter = CompactMapIter<'a, K, V>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, btree_map};

    use proptest::{collection, prelude::*};

    use super::{CompactMap, Entry};

    const SMALL_CAPACITY: usize = 4;

    #[derive(Clone, Debug)]
    enum MapOperation {
        Insert(u8, u8),
        Remove(u8),
        Get(u8),
        Upsert(u8, u8),
        RemoveViaEntry(u8),
    }

    fn map_operation() -> impl Strategy<Value = MapOperation> {
        prop_oneof![
            (0u8..12, any::<u8>()).prop_map(|(key, value)| MapOperation::Insert(key, value)),
            (0u8..12).prop_map(MapOperation::Remove),
            (0u8..12).prop_map(MapOperation::Get),
            (0u8..12, any::<u8>()).prop_map(|(key, value)| MapOperation::Upsert(key, value)),
            (0u8..12).prop_map(MapOperation::RemoveViaEntry),
        ]
    }

    proptest! {
        #[test]
        fn compact_map_matches_btree_map(
            initial in collection::vec((0u8..12, any::<u8>()), 0..24),
            operations in collection::vec(map_operation(), 0..100),
        ) {
            let mut actual = initial.iter().copied().collect::<CompactMap<u8, u8, SMALL_CAPACITY>>();
            let mut model = initial.into_iter().collect::<BTreeMap<_, _>>();

            prop_assert_eq!(
                actual.iter().map(|(key, value)| (*key, *value)).collect::<Vec<_>>(),
                model.iter().map(|(key, value)| (*key, *value)).collect::<Vec<_>>(),
            );

            for operation in operations {
                match operation {
                    MapOperation::Insert(key, value) => {
                        prop_assert_eq!(actual.insert(key, value), model.insert(key, value));
                    }
                    MapOperation::Remove(key) => {
                        prop_assert_eq!(actual.remove(&key), model.remove(&key));
                    }
                    MapOperation::Get(key) => {
                        prop_assert_eq!(actual.get(&key), model.get(&key));
                        prop_assert_eq!(actual.contains_key(&key), model.contains_key(&key));
                    }
                    MapOperation::Upsert(key, value) => {
                        match actual.entry(key) {
                            Entry::Occupied(entry) => {
                                let existing = entry.into_mut();
                                *existing = existing.wrapping_add(value);
                            }
                            Entry::Vacant(entry) => {
                                entry.insert(value);
                            }
                        }
                        match model.entry(key) {
                            btree_map::Entry::Occupied(mut entry) => {
                                let existing = entry.get_mut();
                                *existing = existing.wrapping_add(value);
                            }
                            btree_map::Entry::Vacant(entry) => {
                                entry.insert(value);
                            }
                        }
                    }
                    MapOperation::RemoveViaEntry(key) => {
                        let actual_removed = match actual.entry(key) {
                            Entry::Occupied(entry) => Some(entry.remove()),
                            Entry::Vacant(_) => None,
                        };
                        let model_removed = match model.entry(key) {
                            btree_map::Entry::Occupied(entry) => Some(entry.remove()),
                            btree_map::Entry::Vacant(_) => None,
                        };
                        prop_assert_eq!(actual_removed, model_removed);
                    }
                }

                prop_assert_eq!(actual.len(), model.len());
                prop_assert_eq!(actual.is_empty(), model.is_empty());
                prop_assert_eq!(
                    actual.iter().map(|(key, value)| (*key, *value)).collect::<Vec<_>>(),
                    model.iter().map(|(key, value)| (*key, *value)).collect::<Vec<_>>(),
                );
            }

            prop_assert_eq!(
                (&actual).into_iter().map(|(key, value)| (*key, *value)).collect::<Vec<_>>(),
                model.iter().map(|(key, value)| (*key, *value)).collect::<Vec<_>>(),
            );
            prop_assert_eq!(actual.clone().into_iter().collect::<Vec<_>>(), model.clone().into_iter().collect::<Vec<_>>());
        }
    }
}
