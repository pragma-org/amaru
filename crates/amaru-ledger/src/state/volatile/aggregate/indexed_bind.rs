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

use crate::state::volatile::{Bind, DiffBind, Existence};

/// A per-key log of `Bind` verdicts, indexed so that folding one key's history is cheap and
/// retracting the oldest contribution is exact.
///
/// A [`DiffBind`] represents a single change: the bindings one fragment applied to a set of keys.
/// An `IndexedBind` tracks *many* such changes: for each key it keeps the ordered sequence of
/// per-fragment verdicts that touched it, newest at the front. Each verdict is an [`Existence`]:
///
/// - `Exists(bind)`: the fragment (re-)registered the key, or updated its bindings;
/// - `Gone`: the fragment unregistered the key.
///
/// A fragment that doesn't touch a key adds nothing to its deque, so `Unknown` is never stored.
///
/// Folding those changes down into a single `DiffBind` would still answer reads, but it collapses
/// the sequence into one flat state and forgets which fragment contributed what; keeping them
/// separate per key is what makes retracting the oldest fragment exact.
///
/// This buys exact incremental remove: retracting the oldest fragment pops the back of
/// each touched key's deque, meaning there is no need to recompute an aggregate.
///
/// The cost is paid on read, where [`get`](Self::get) folds a key's history back into a single verdict.
#[derive(Debug, Clone)]
pub struct IndexedBind<K: Ord, L, R, V> {
    index: BTreeMap<K, VecDeque<Existence<Bind<L, R, V>>>>,
}

impl<K: Ord, L, R, V> Default for IndexedBind<K, L, R, V> {
    fn default() -> Self {
        Self { index: BTreeMap::new() }
    }
}

impl<K: Ord, L, R, V> IndexedBind<K, L, R, V> {
    /// Iter over all known records.
    pub fn iter(&self) -> impl Iterator<Item = (&K, Existence<Bind<&L, &R, &V>>)> {
        self.index.keys().map(|key| (key, self.get(key)))
    }

    /// Append a fragment's bindings, treating them as applied *after* everything already recorded.
    /// Each registered key gains an `Exists` verdict at the back of its deque; each unregistered
    /// key gains a `Gone` tombstone.
    pub fn extend<const MAP_CAPACITY: usize, const SET_CAPACITY: usize>(
        &mut self,
        diff: &DiffBind<K, L, R, V, MAP_CAPACITY, SET_CAPACITY>,
    ) where
        K: ToOwned<Owned = K>,
        L: ToOwned<Owned = L>,
        R: ToOwned<Owned = R>,
        V: ToOwned<Owned = V>,
    {
        for (key, bind) in &diff.registered {
            push_front_or_insert(&mut self.index, key, Existence::Exists(bind.to_owned()));
        }

        for key in &diff.unregistered {
            push_front_or_insert(&mut self.index, key, Existence::Gone);
        }
    }

    /// Like [`Self::extend`] but allows transforming a bind when indexing.
    pub fn extend_with<Lsrc, Rsrc, Vsrc, const MAP_CAPACITY: usize, const SET_CAPACITY: usize>(
        &mut self,
        diff: &DiffBind<K, Lsrc, Rsrc, Vsrc, MAP_CAPACITY, SET_CAPACITY>,
        with: impl Fn(Bind<Lsrc, Rsrc, Vsrc>) -> Bind<L, R, V>,
    ) where
        K: ToOwned<Owned = K>,
        Lsrc: ToOwned<Owned = Lsrc>,
        Rsrc: ToOwned<Owned = Rsrc>,
        Vsrc: ToOwned<Owned = Vsrc>,
    {
        for (key, bind) in &diff.registered {
            push_front_or_insert(&mut self.index, key, Existence::Exists(with(bind.to_owned())));
        }

        for key in &diff.unregistered {
            push_front_or_insert(&mut self.index, key, Existence::Gone);
        }
    }

    /// Retract the oldest fragment's contribution for every key it touched, popping the front of
    /// each deque and dropping the key once its history empties.
    pub fn remove<Lany, Rany, Vany, const MAP_CAPACITY: usize, const SET_CAPACITY: usize>(
        &mut self,
        diff: &DiffBind<K, Lany, Rany, Vany, MAP_CAPACITY, SET_CAPACITY>,
    ) -> bool {
        let mut all_present = true;

        for key in diff.registered.keys().chain(diff.unregistered.iter()) {
            match self.index.get_mut(key) {
                Some(contributions) => {
                    contributions.pop_back();
                    if contributions.is_empty() {
                        self.index.remove(key);
                    }
                }
                None => all_present = false,
            }
        }

        all_present
    }

    /// Fold a key's per-fragment history into a single verdict.
    ///
    /// Folding, rather than reading only the newest entry, is required because a bind-only update
    /// (`value: None`, e.g. a lone delegation change) is partial: it must compose with the
    /// registration established by an earlier fragment. `Existence::or_else_bind` performs exactly
    /// that composition. `Gone` and full re-registrations supersede, bind-only updates merge onto
    /// what came before.
    pub fn get(&self, key: &K) -> Existence<Bind<&L, &R, &V>> {
        match self.index.get(key) {
            None => Existence::Unknown,
            Some(seq) => Existence::fold(seq.iter().map(|existence| existence.as_refs())),
        }
    }

    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.index.is_empty()
    }
}

fn push_front_or_insert<K, L, R, V>(
    index: &mut BTreeMap<K, VecDeque<Existence<Bind<L, R, V>>>>,
    key: &K,
    value: Existence<Bind<L, R, V>>,
) where
    K: Ord + ToOwned<Owned = K>,
{
    match index.get_mut(key) {
        Some(seq) => {
            seq.push_front(value);
        }
        None => {
            index.insert((*key).to_owned(), VecDeque::from([value]));
        }
    };
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::state::volatile::{Resettable, any_diff_bind};

    #[test]
    fn get_composes_diff_bind_in_the_right_order() {
        let mut register = DiffBind::<u8, u8, u8, u8>::default();
        register.register(1, 100, Some(10), None).unwrap();

        let mut delegate = DiffBind::<u8, u8, u8, u8>::default();
        delegate.bind_left(1, Some(20)).unwrap();

        let mut register_then_delegate = IndexedBind::default();
        register_then_delegate.extend(&register);
        register_then_delegate.extend(&delegate);

        match register_then_delegate.get(&1) {
            Existence::Exists(bind) => {
                assert_eq!(bind.value, Some(&100));
                assert_eq!(bind.left, Resettable::Set(&20));
            }
            other @ (Existence::Gone | Existence::Unknown) => panic!("expected a merged registration, got {other:?}"),
        }

        let mut delegate_then_register = IndexedBind::default();
        delegate_then_register.extend(&delegate);
        delegate_then_register.extend(&register);

        match delegate_then_register.get(&1) {
            Existence::Exists(bind) => {
                assert_eq!(bind.value, Some(&100));
                assert_eq!(bind.left, Resettable::Set(&10));
            }
            other @ (Existence::Gone | Existence::Unknown) => panic!("expected a merged registration, got {other:?}"),
        }
    }

    proptest! {
        // Resolving a key from its indexed history must equal folding the same fragments into a
        // single `DiffBind` and looking the key up. This ties the per-key incremental structure to
        // the flat fold it stands in for.
        #[test]
        fn resolve_matches_diff_bind_fold(
            window in prop::collection::vec(any_diff_bind(), 1..6),
            do_remove in any::<bool>(),
        ) {
            let mut indexed = IndexedBind::default();
            for diff in &window {
                indexed.extend(diff);
            }

            let folded = if do_remove {
                let (first, rest) = window.split_first().expect("non-empty window");
                assert!(indexed.remove(first));
                DiffBind::fold(rest.iter()).owned()
            } else {
                DiffBind::fold(window.iter()).owned()
            };

            for key in 0..u8::MAX {
                prop_assert_eq!(indexed.get(&key), folded.get(&key));
            }
        }

        /// Extending a window of fragments then retracting them oldest-first must leave the index
        /// empty.
        #[test]
        fn extend_then_remove_is_empty(window in prop::collection::vec(any_diff_bind(), 0..6)) {
            let mut indexed = IndexedBind::default();

            for diff in &window {
                indexed.extend(diff);
            }

            for diff in &window {
                assert!(indexed.remove(diff));
            }

            prop_assert!(indexed.is_empty());
        }
    }
}
