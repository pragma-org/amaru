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

use amaru_kernel::{CompactMap, CompactSet};

use crate::state::volatile::Existence;

/// A compact data-structure tracking changes in a DAG. A composition relation exists, allowing to reduce
/// two `DiffSet` into one that is equivalent to applying both `DiffSet` in sequence.
///
/// Concretely, we use this to track changes to apply to the UTxO set across a block, coming from
/// the processing of each transaction in sequence.
///
/// `PRODUCED` and `CONSUMED` are the promotion thresholds of the backing compact collections; the
/// defaults promote immediately, behaving like plain B-trees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffSet<K: Ord, V, const PRODUCED: usize = 0, const CONSUMED: usize = 0> {
    /// Keys consumed by this diff.
    pub consumed: CompactSet<K, CONSUMED>,
    /// Keys produced by this diff together with their resulting values.
    pub produced: CompactMap<K, V, PRODUCED>,
}

impl<K: Ord, V, const PRODUCED: usize, const CONSUMED: usize> Default for DiffSet<K, V, PRODUCED, CONSUMED> {
    fn default() -> Self {
        Self { consumed: Default::default(), produced: Default::default() }
    }
}

impl<K: Ord, V, const PRODUCED: usize, const CONSUMED: usize> DiffSet<K, V, PRODUCED, CONSUMED> {
    /// Borrow all keys and values in this diff.
    pub fn as_refs(&self) -> DiffSet<&K, &V> {
        DiffSet { consumed: self.consumed.iter().collect(), produced: self.produced.iter().collect() }
    }

    /// Lookup the state associated to a key, if any. Returns `Existence::Unknown` if the state
    /// cannot be determined from the available data.
    pub fn get<'a>(&'a self, k: &K) -> Existence<&'a V> {
        if let Some(v) = self.produced.get(k) {
            Existence::Exists(v)
        } else if self.consumed.contains(k) {
            Existence::Gone
        } else {
            Existence::Unknown
        }
    }

    /// Record that this diff produces `k` with value `v`.
    pub fn produce(&mut self, k: K, v: V) {
        self.produced.insert(k, v);
    }

    /// Record that this diff consumes `k`, cancelling any prior production in the same diff.
    pub fn consume(&mut self, k: K) {
        self.produced.remove(&k);
        self.consumed.insert(k);
    }

    /// Merge another diff into this one, assuming `other` happened later.
    pub fn extend<const P: usize, const C: usize>(&mut self, other: &DiffSet<K, V, P, C>)
    where
        // TODO: lower requirement to 'Copy' for DiffSet keys
        //
        // This needs to be clone because `TransactionInput` isn't `Copy` at the moment. But
        // it's reasonable to ask keys to be always Copy in this scenario.
        K: ToOwned<Owned = K>,
        V: ToOwned<Owned = V>,
    {
        for k in &other.consumed {
            self.produced.remove(k);
            self.consumed.insert(k.to_owned());
        }

        for (k, v) in &other.produced {
            self.produced.insert(k.to_owned(), v.to_owned());
        }
    }

    /// Remove the effect of a previous `DiffSet` on the current `DiffSet`. This is technically an
    /// `undo` operation, but with the extra assumption that something consumed is never produced
    /// again.
    ///
    /// An important consideration is also that this function's goal is not to exactly revert a
    /// `DiffSet`, but rather, to cleanup memory as much as we can in a cheap way; this ensures
    /// that one can use a `DiffSet` as a cache, while keeping the memory under control.
    pub fn remove<const P: usize, const C: usize>(&mut self, other: &DiffSet<K, V, P, C>) {
        for k in other.produced.keys() {
            self.produced.remove(k);
        }

        for k in &other.consumed {
            self.consumed.remove(k);
        }
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use proptest::prelude::*;

    use super::*;

    prop_compose! {
        pub fn any_diff_set()(
            consumed in
                any::<BTreeSet<u8>>(),
            mut produced in
                any::<BTreeMap<u8, u8>>()
        ) -> DiffSet<u8, u8> {
            produced.retain(|k, _| !consumed.contains(k));
            DiffSet {
                produced: produced.into_iter().collect(),
                consumed: consumed.into_iter().collect(),
            }
        }
    }

    proptest! {
        #[test]
        fn prop_extend_itself(st in any_diff_set()) {
            let mut original = st.clone();
            original.extend(&st);
            prop_assert_eq!(st, original);
        }
    }

    proptest! {
        #[test]
        fn prop_merge_no_overlap(mut st in any_diff_set(), mut diff in any_diff_set()) {
            // Extra assumptions that must hold for this property:
            //
            // - We cannot produce an item produced or consumed before
            // - We cannot consume an item twice
            diff.produced = std::mem::take(&mut diff.produced)
                .into_iter()
                .filter(|(k, _)| !st.produced.contains_key(k) && !st.consumed.contains(k))
                .collect();
            diff.consumed =
                std::mem::take(&mut diff.consumed).into_iter().filter(|k| !st.consumed.contains(k)).collect();

            st.extend(&diff);

            for (k, v) in diff.produced.iter() {
                prop_assert_eq!(
                    st.produced.get(k),
                    Some(v),
                    "everything newly produced is produced"
                );
            }

            for k in diff.consumed.iter() {
                prop_assert!(
                    st.consumed.contains(k),
                    "everything newly consumed is consumed",
                );
            }

            for k in st.produced.keys() {
                prop_assert!(
                    !st.consumed.contains(k),
                    "nothing produced is also consumed",
                )
            }

            for k in st.consumed.iter() {
                prop_assert!(
                    !st.produced.contains_key(k),
                    "nothing consumed is also produced",
                )
            }
        }
    }

    proptest! {
        #[test]
        fn prop_composition(
            st0 in any_diff_set().prop_map(|st| st.produced.into_iter().collect::<BTreeMap<_, _>>()),
            diffs in prop::collection::vec(any_diff_set(), 1..5),
        ) {
            // NOTE: The order in which we apply transformation here doesn't matter, because we
            // know that DiffSet consumed and produced do not overlap _by construction_ (cf the
            // prop_merge_no_overlap). So we could write the two statements below in any order.
            fn apply(mut st: BTreeMap<u8, u8>, diff: &DiffSet<u8, u8>) -> BTreeMap<u8, u8> {
                for k in diff.consumed.iter() {
                    st.remove(k);
                }

                for (k, v) in diff.produced.iter() {
                    st.insert(*k, *v);
                }

                st
            }

            // Apply each diff in sequence.
            let st_seq = diffs.iter().fold(st0.clone(), apply);

            // Apply a single reduced diff
            let st_compose = apply(
                st0,
                &diffs
                    .into_iter()
                    .fold(DiffSet::default(), |mut acc, diff| {
                        acc.extend(&diff);
                        acc
                    })
            );

            assert_eq!(st_seq, st_compose);
        }
    }

    #[test]
    fn lookup_resolves_existence() {
        use crate::state::volatile::Existence;

        let mut diff = DiffSet::<u8, u8>::default();
        diff.produce(1, 100);
        diff.consume(2);

        assert!(matches!(diff.get(&1), Existence::Exists(&100)));
        assert!(matches!(diff.get(&2), Existence::Gone));
        assert!(matches!(diff.get(&3), Existence::Unknown));
    }
}
