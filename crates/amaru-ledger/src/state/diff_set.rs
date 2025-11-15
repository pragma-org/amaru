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

use std::collections::{BTreeMap, BTreeSet};

/// A compact data-structure tracking changes in a DAG. A composition relation exists, allowing to reduce
/// two `DiffSet` into one that is equivalent to applying both `DiffSet` in sequence.
///
/// Concretely, we use this to track changes to apply to the UTxO set across a block, coming from
/// the processing of each transaction in sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffSet<K: Ord, V> {
    pub consumed: BTreeSet<K>,
    pub produced: BTreeMap<K, V>,
}

impl<K: Ord, V> Default for DiffSet<K, V> {
    fn default() -> Self {
        Self {
            consumed: Default::default(),
            produced: Default::default(),
        }
    }
}

impl<K: Ord, V> DiffSet<K, V> {
    pub fn merge(&mut self, other: Self) {
        self.produced.retain(|k, _| !other.consumed.contains(k));
        self.consumed.retain(|k| !other.produced.contains_key(k));
        self.consumed.extend(other.consumed);
        self.produced.extend(other.produced);
    }

    pub fn produce(&mut self, k: K, v: V) {
        self.produced.insert(k, v);
    }

    pub fn consume(&mut self, k: K) -> (&K, Option<V>) {
        let removed = self.produced.remove(&k);
        let entry = self.consumed.get_or_insert(k);
        (entry, removed)
    }

    pub fn as_ref(&self) -> DiffSet<&K, &V> {
        DiffSet {
            consumed: self.consumed.iter().collect(),
            produced: self.produced.iter().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::collections::{BTreeMap, BTreeSet};

    prop_compose! {
        fn any_diff()(
            consumed in
                any::<BTreeSet<u8>>(),
            mut produced in
                any::<BTreeMap<u8, u8>>()
        ) -> DiffSet<u8, u8> {
            produced.retain(|k, _| !consumed.contains(k));
            DiffSet {
                produced,
                consumed,
            }
        }
    }

    proptest! {
        #[test]
        fn prop_merge_itself(mut st in any_diff()) {
            let original = st.clone();
            st.merge(st.clone());
            prop_assert_eq!(st, original);
        }
    }

    proptest! {
        #[test]
        fn prop_merge_no_overlap(mut st in any_diff(), diff in any_diff()) {
            st.merge(diff.clone());

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

            for (k, _) in st.produced.iter() {
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
            st0 in any_diff().prop_map(|st| st.produced),
            diffs in prop::collection::vec(any_diff(), 1..5),
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
                        acc.merge(diff);
                        acc
                    })
            );

            assert_eq!(st_seq, st_compose);
        }
    }
}
