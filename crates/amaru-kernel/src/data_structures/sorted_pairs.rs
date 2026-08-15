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

use std::{borrow::Borrow, collections::BTreeMap, fmt};

/// A _near drop-in_ replacement for BTreeMap but with its internal based on a vector. The API is
/// purposely crafted to reduce and control memory-allocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SortedPairs<K: Ord, V>(Vec<(K, V)>);

/// Construct an empty sequence; should generally be avoided in favor of `Self::with_capacity`.
impl<K: Ord, V> Default for SortedPairs<K, V> {
    fn default() -> Self {
        Self(Vec::default())
    }
}

/// A From instance that is mostly convenient for testing, otherwise defies the purpose of the
/// entire structure.
impl<K: Ord, V> From<BTreeMap<K, V>> for SortedPairs<K, V> {
    fn from(map: BTreeMap<K, V>) -> Self {
        Self(map.into_iter().collect())
    }
}

impl<K: Ord, V> SortedPairs<K, V> {
    /// Like `Self::push`, but takes and give back ownership to help with builder-style API.
    pub fn and_push(mut self, k: K, v: V) -> Self
    where
        K: fmt::Debug,
    {
        self.push(k, v);
        self
    }

    /// Concatenate two pairs together.
    ///
    /// Panics if the first key of the right-hand side is not strictly after the last key of the
    /// left-hand side.
    pub fn append(mut self, mut rhs: Self) -> Self
    where
        K: fmt::Debug,
    {
        if let Some((lhs_last, _)) = self.0.last()
            && let Some((rhs_first, _)) = rhs.0.first()
        {
            assert!(
                rhs_first > lhs_last,
                "invariant violation (SortedPairs.append): rhs' first element ({:?}) is not after lhs' last element ({:?})",
                rhs_first,
                lhs_last,
            );
        }

        self.0.append(&mut rhs.0);
        self
    }

    /// Returns true if the pairs contains a value for the specified key.
    ///
    /// The key may be any borrowed form of the pairs’ key type, but the ordering on the borrowed form must match the ordering on the key type.
    pub fn contains_key<Q>(&self, k: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.binary_search(k).is_ok()
    }

    /// Returns a reference to the value corresponding to the key.
    ///
    /// The key may be any borrowed form of the pairs’ key type, but the ordering on the borrowed
    /// form must match the ordering on the key type.
    pub fn get<Q>(&self, k: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        let ix = self.binary_search(k).ok()?;
        Some(&self.0[ix].1)
    }

    /// Returns a **mutable** reference to the value corresponding to the key.
    ///
    /// The key may be any borrowed form of the pairs’s key type, but the ordering on the borrowed
    /// form must match the ordering on the key type.
    pub fn get_mut<Q>(&mut self, k: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        let ix = self.binary_search(k).ok()?;
        Some(&mut self.0[ix].1)
    }

    /// Insert at key/value pairs at the right location replacing any existing key.
    ///
    /// This method can be costly as it will need to re-allocate and move elements around for
    /// values inserted in the middle of the sequence.
    pub fn insert(&mut self, k: K, v: V) {
        match self.binary_search(&k) {
            Ok(ix) => {
                self.0[ix].1 = v;
            }
            Err(ix) => {
                self.0.insert(ix, (k, v));
            }
        }
    }

    /// Returns `true` if there are no pairs.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Gets an iterator over the entries of the pairs, sorted by key.
    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.0.iter().map(|(k, v)| (k, v))
    }

    /// Gets an iterator over mutable entries of the pairs, sorted by key.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&K, &mut V)> {
        self.0.iter_mut().map(|(k, v)| (&*k, v))
    }

    /// Gets an iterator over the keys of the pairs, in order by key.
    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.0.iter().map(|(k, _v)| k)
    }

    /// Returns the number of pairs.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Add a new key/value pair after existing keys.
    ///
    /// Panics if the key is not strictly after the last inserted one.
    pub fn push(&mut self, k: K, v: V)
    where
        K: fmt::Debug,
    {
        if let Some((last, _)) = self.0.last() {
            assert!(
                &k > last,
                "invariant violation (SortedPairs.push): pushed element ({k:?}) is not after last element ({last:?})",
            );
        }

        self.0.push((k, v));
    }

    /// Gets an iterator over the values of the pairs, in order by key.
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.0.iter().map(|(_k, v)| v)
    }

    /// Constructs a new, empty `SortedPairs<K, V> with at least the specified capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self(Vec::with_capacity(capacity))
    }
}

impl<K: Ord, V> SortedPairs<K, V> {
    fn binary_search<Q>(&self, k: &Q) -> Result<usize, usize>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.0.binary_search_by_key(&k, |(k, _)| k.borrow())
    }
}

#[cfg(test)]
mod tests {
    use proptest::{collection::btree_map, prelude::*};

    use crate::SortedPairs;

    proptest! {
        #[test]
        fn behave_similar_to_btreemap(map in btree_map(any::<u8>(), any::<bool>(), 0..10)) {
            let mut pairs = SortedPairs::with_capacity(map.len());
            for (k, v) in map.iter() {
                pairs.push(*k, *v);
            }

            prop_assert_eq!(&SortedPairs::from(map.clone()), &pairs, "from");

            prop_assert_eq!(pairs.len(), map.len());
            prop_assert_eq!(pairs.is_empty(), map.is_empty());

            for (l, r) in map.iter().zip(pairs.iter()) {
                prop_assert_eq!(map.contains_key(l.0), pairs.contains_key(l.0), ".contains_key");
                prop_assert_eq!(pairs.get(r.0), Some(l.1), ".get");
                prop_assert_eq!(l, r, ".iter/.zip");
            }

            prop_assert_eq!(map.keys().collect::<Vec<_>>(), pairs.keys().collect::<Vec<_>>(), ".keys");
            prop_assert_eq!(map.values().collect::<Vec<_>>(), pairs.values().collect::<Vec<_>>(), ".values");
        }
    }
}
