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

use std::collections::{BTreeMap, VecDeque};

use crate::state::volatile::{DiffSet, Existence};

/// A per-key log of set verdicts, indexed so that reading one key is cheap and retracting the oldest
/// contribution is exact.
///
/// A [`DiffSet`] represents a single change: the keys one fragment produced or consumed. An
/// `IndexedSet` tracks *many* such changes: for each key it keeps the ordered sequence of
/// per-fragment verdicts that touched it, oldest at the front. Each verdict is an [`Existence`]:
///
/// - `Exists(value)`: the fragment produced the key with that value;
/// - `Gone`: the fragment consumed the key.
///
/// A fragment that doesn't touch a key adds nothing to its deque, so `Unknown` is never stored.
///
/// This is to [`DiffSet`] what [`crate::state::indexed_bind::IndexedBind`] is to
/// [`crate::state::diff_bind::DiffBind`]: unlike a bind, a set has no partial updates, so every
/// verdict is absolute and [`resolve`](Self::resolve) reads the newest one directly rather than
/// folding a history.
///
/// Collapsing the changes down into a single `DiffSet` would still answer reads, but blind cleanup
/// only stays exact when a consumed key is never produced again (as for UTxOs); keeping the verdicts
/// separate per key is what makes retracting the oldest fragment exact even when a key repeats.
#[derive(Debug, Clone)]
pub struct IndexedSet<K: Ord, V> {
    index: BTreeMap<K, VecDeque<Existence<V>>>,
}

impl<K: Ord, V> Default for IndexedSet<K, V> {
    fn default() -> Self {
        Self { index: BTreeMap::new() }
    }
}

impl<K: Ord, V> IndexedSet<K, V> {
    /// Append a fragment's changes, treating them as applied *after* everything already recorded.
    /// Each consumed key gains a `Gone` tombstone and each produced key gains an `Exists` verdict at
    /// the back of its deque; produced is applied last so it wins the newest slot, mirroring
    /// [`DiffSet::lookup`].
    pub fn extend(&mut self, diff: &DiffSet<K, V>)
    where
        K: Clone,
        V: Clone,
    {
        for key in &diff.consumed {
            self.index.entry(key.clone()).or_default().push_back(Existence::Gone);
        }
        for (key, value) in &diff.produced {
            self.index.entry(key.clone()).or_default().push_back(Existence::Exists(value.clone()));
        }
    }

    /// Retract the oldest fragment's contribution for every key it touched, popping the front of
    /// each deque and dropping the key once its history empties.
    pub fn cleanup(&mut self, diff: &DiffSet<K, V>) {
        for key in diff.consumed.iter().chain(diff.produced.keys()) {
            match self.index.get_mut(key) {
                Some(contributions) => {
                    contributions.pop_front();
                    if contributions.is_empty() {
                        self.index.remove(key);
                    }
                }
                None => unreachable!("retracted a fragment touching a key absent from the aggregate"),
            }
        }
    }

    /// This index's verdict on a key: its newest per-fragment contribution, or `Unknown` when no
    /// recorded fragment touched it.
    pub fn resolve(&self, key: &K) -> Existence<V>
    where
        V: Clone,
    {
        match self.index.get(key).and_then(|contributions| contributions.back()) {
            Some(verdict) => verdict.clone(),
            None => Existence::Unknown,
        }
    }
    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.index.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::state::volatile::any_diff_set;

    fn fold(window: &[DiffSet<u8, u8>]) -> DiffSet<u8, u8> {
        let mut acc = DiffSet::default();
        for diff in window {
            acc.extend(diff);
        }
        acc
    }

    fn as_parts(verdict: Existence<u8>) -> Option<Option<u8>> {
        match verdict {
            Existence::Exists(value) => Some(Some(value)),
            Existence::Gone => Some(None),
            Existence::Unknown => None,
        }
    }

    proptest! {
        /// Resolving a key from its indexed history must equal collapsing the same fragments into a
        /// single `DiffSet` and looking the key up. This ties the per-key structure to the flat
        /// collapse it stands in for.
        #[test]
        fn resolve_matches_diff_set_lookup(window in prop::collection::vec(any_diff_set(), 0..6)) {
            let mut indexed = IndexedSet::default();
            for diff in &window {
                indexed.extend(diff);
            }

            let folded = fold(&window);

            for key in 0u8..8 {
                prop_assert_eq!(as_parts(indexed.resolve(&key)), as_parts(folded.lookup(&key).copied()));
            }
        }

        /// Retracting the oldest fragment must leave reads equal to collapsing only the fragments
        /// that remain. This is the exactness a blind collapse loses when a key repeats.
        #[test]
        fn resolve_after_retracting_oldest_matches_remaining(
            window in prop::collection::vec(any_diff_set(), 1..6),
        ) {
            let mut indexed = IndexedSet::default();
            for diff in &window {
                indexed.extend(diff);
            }

            indexed.cleanup(&window[0]);

            let folded = fold(&window[1..]);

            for key in 0u8..8 {
                prop_assert_eq!(as_parts(indexed.resolve(&key)), as_parts(folded.lookup(&key).copied()));
            }
        }

        //        /// Retracting the newest fragment must leave reads equal to collapsing only the fragments
        //        /// that remain. This is the rollback-side mirror of retracting the oldest, and the
        //        /// exactness a blind collapse loses when a key repeats.
        //        #[test]
        //        fn resolve_after_retracting_newest_matches_remaining(
        //            window in prop::collection::vec(any_diff_set(), 1..6),
        //        ) {
        //            let mut indexed = IndexedSet::default();
        //            for diff in &window {
        //                indexed.extend(diff);
        //            }
        //
        //            let (last, rest) = window.split_last().expect("non-empty window");
        //            indexed.pop_back(last);
        //
        //            let folded = fold(rest);
        //
        //            for key in 0u8..8 {
        //                prop_assert_eq!(as_parts(indexed.resolve(&key)), as_parts(folded.lookup(&key).copied()));
        //            }
        //        }

        /// Extending a window of fragments then retracting them oldest-first must leave the index
        /// empty.
        #[test]
        fn extend_then_cleanup_is_empty(window in prop::collection::vec(any_diff_set(), 0..6)) {
            let mut indexed = IndexedSet::default();
            for diff in &window {
                indexed.extend(diff);
            }
            for diff in &window {
                indexed.cleanup(diff);
            }

            prop_assert!(indexed.is_empty());
        }

        //        /// Extending a window of fragments then retracting them newest-first must leave the index
        //        /// empty. This is what rolling the whole window back relies on.
        //        #[test]
        //        fn extend_then_pop_back_is_empty(window in prop::collection::vec(any_diff_set(), 0..6)) {
        //            let mut indexed = IndexedSet::default();
        //            for diff in &window {
        //                indexed.extend(diff);
        //            }
        //            for diff in window.iter().rev() {
        //                indexed.pop_back(diff);
        //            }
        //
        //            prop_assert!(indexed.is_empty());
        //        }
    }
}
