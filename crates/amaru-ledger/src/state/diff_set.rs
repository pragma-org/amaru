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

use crate::state::{diff_bind::DiffBind, volatile::Existence};

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
        Self { consumed: Default::default(), produced: Default::default() }
    }
}

impl<K: Ord, V> DiffSet<K, V> {
    pub fn extend(&mut self, other: &DiffSet<K, V>)
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

    /// Like `Self::extend`, but ignores bind left and right.
    pub fn extend_bind<L, R>(&mut self, other: &DiffBind<K, L, R, V>)
    where
        // TODO: lower requirement to 'Copy' for DiffSet keys
        //
        // This needs to be clone because `TransactionInput` isn't `Copy` at the moment. But
        // it's reasonable to ask keys to be always Copy in this scenario.
        K: ToOwned<Owned = K>,
        V: ToOwned<Owned = V>,
    {
        for k in &other.unregistered {
            self.produced.remove(k);
            self.consumed.insert(k.to_owned());
        }

        for (k, bind) in &other.registered {
            if let Some(v) = bind.value.as_ref() {
                self.produced.insert(k.to_owned(), v.to_owned());
            }
        }
    }

    /// Lookup the state associated to a key, if any. Returns `Existence::Unknown` if the state
    /// cannot be determined from the available data.
    pub fn lookup<'a>(&'a self, k: &K) -> Existence<&'a V> {
        if let Some(v) = self.produced.get(k) {
            Existence::Exists(v)
        } else if self.consumed.contains(k) {
            Existence::Gone
        } else {
            Existence::Unknown
        }
    }

    /// Remove the effect of a previous `DiffSet` on the current `DiffSet`. This is technically an
    /// `undo` operation, but with the extra assumption that something consumed is never produced
    /// again.
    ///
    /// An important consideration is also that this function's goal is not to exactly revert a
    /// `DiffSet`, but rather, to cleanup memory as much as we can in a cheap way; this ensures
    /// that one can use a `DiffSet` as a cache, while keeping the memory under control.
    pub fn cleanup(&mut self, other: &DiffSet<K, V>) {
        for k in other.produced.keys() {
            self.produced.remove(k);
        }

        for k in &other.consumed {
            self.consumed.remove(k);
        }
    }

    /// Like `Self::cleanup`, but from a `DiffBind` interpreted as a `DiffSet`. The left and right
    /// binds are ignored, and we only treat registered event with a value as having an effect.
    pub fn cleanup_bind<L, R>(&mut self, other: &DiffBind<K, L, R, V>) {
        for (k, bind) in &other.registered {
            if bind.value.is_some() {
                self.produced.remove(k);
            }
        }

        for k in &other.unregistered {
            self.consumed.remove(k);
        }
    }

    pub fn produce(&mut self, k: K, v: V) {
        self.produced.insert(k, v);
    }

    pub fn consume(&mut self, k: K) {
        self.produced.remove(&k);
        self.consumed.insert(k);
    }

    pub fn as_ref(&self) -> DiffSet<&K, &V> {
        DiffSet { consumed: self.consumed.iter().collect(), produced: self.produced.iter().collect() }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use proptest::prelude::*;

    use super::*;
    use crate::state::{
        diff_bind::{DiffBind, test_support::arbitrary_diff_bind},
        volatile::Existence,
    };

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
        fn prop_extend_itself(st in any_diff()) {
            let mut original = st.clone();
            original.extend(&st);
            prop_assert_eq!(st, original);
        }
    }

    proptest! {
        #[test]
        fn prop_merge_no_overlap(mut st in any_diff(), mut diff in any_diff()) {
            // Extra assumptions that must hold for this property:
            //
            // - We cannot produce an item produced or consumed before
            // - We cannot consume an item twice
            diff.produced.retain(|k, _| !st.produced.contains_key(k) && !st.consumed.contains(k));
            diff.consumed.retain(|k| !st.consumed.contains(k));

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
                        acc.extend(&diff);
                        acc
                    })
            );

            assert_eq!(st_seq, st_compose);
        }
    }

    #[test]
    fn lookup_resolves_existence() {
        let mut diff = DiffSet::<u8, u8>::default();
        diff.produce(1, 100);
        diff.consume(2);

        assert!(matches!(diff.lookup(&1), Existence::Exists(&100)));
        assert!(matches!(diff.lookup(&2), Existence::Gone));
        assert!(matches!(diff.lookup(&3), Existence::Unknown));
    }

    #[test]
    fn extend_bind_projects_registrations_and_unregistrations() {
        let mut set = DiffSet::<u8, u8>::default();
        set.produce(3, 30);

        let mut bind = DiffBind::<u8, (), (), u8>::default();
        bind.register(1, 100, None, None).unwrap();
        bind.unregister(3);

        set.extend_bind(&bind);

        assert_eq!(set.produced.get(&1), Some(&100));
        assert!(!set.produced.contains_key(&3));
        assert!(set.consumed.contains(&3));
    }

    #[test]
    fn extend_bind_ignores_bind_only_updates() {
        let mut bind = DiffBind::<u8, u8, u8, u8>::default();
        bind.bind_left(1, Some(10)).unwrap();
        bind.bind_right(2, Some(20)).unwrap();

        let mut set = DiffSet::<u8, u8>::default();
        set.extend_bind(&bind);

        assert!(set.produced.is_empty());
        assert!(set.consumed.is_empty());
    }

    proptest! {
        /// `extend_bind` folds a fragment's bindings into the aggregate DiffSet; `cleanup_bind`
        /// retracts them. Applied in sequence over keys disjoint from the base, they must cancel
        /// out. This is exactly what `VolatileAggregate::{add_fragment, remove_fragment}` rely on.
        #[test]
        fn extend_bind_then_cleanup_bind_is_identity(base in any_diff(), bind in arbitrary_diff_bind()) {
            // Precondition: the bind's keys must be disjoint from the base's, otherwise extend/cleanup would clobber pre-existing base entries.
            let bind_keys: BTreeSet<u8> =
                bind.registered.keys().chain(bind.unregistered.iter()).copied().collect();
            let mut base = base;
            base.produced.retain(|k, _| !bind_keys.contains(k));
            base.consumed.retain(|k| !bind_keys.contains(k));

            let mut roundtrip = base.clone();
            roundtrip.extend_bind(&bind);
            roundtrip.cleanup_bind(&bind);

            prop_assert_eq!(roundtrip, base);
        }
    }
}
