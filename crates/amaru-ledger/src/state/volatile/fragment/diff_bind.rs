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

use amaru_kernel::{CompactMap, CompactSet, compact_collections::Entry};

use crate::state::volatile::{Bind, Existence, Resettable};

/// A compact data-structure tracking changes in a DAG which supports optional linking of values with
/// another data-structure. Items can only be linked if they have been registered first. Yet, they
/// can be unlinked without being unregistered.
///
/// `REGISTERED` and `UNREGISTERED` are the promotion thresholds of the backing compact collections;
/// the defaults promote immediately, behaving like plain B-trees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffBind<K: Ord, L, R, V, const REGISTERED: usize = 0, const UNREGISTERED: usize = 0> {
    /// Keys registered or updated by this diff, together with their pending bindings.
    pub registered: CompactMap<K, Bind<L, R, V>, REGISTERED>,
    /// Keys explicitly removed by this diff.
    pub unregistered: CompactSet<K, UNREGISTERED>,
}

impl<K: Ord, L, R, V, const REGISTERED: usize, const UNREGISTERED: usize> Default
    for DiffBind<K, L, R, V, REGISTERED, UNREGISTERED>
{
    fn default() -> Self {
        Self { registered: Default::default(), unregistered: Default::default() }
    }
}

/// Merge two states together, assuming that the other is a more recent update.
///
/// Importantly, this composes two already-validated `DiffBind`s, it does not re-validate them.
///
/// In particular, a `value: Some(...)` in `most_recent` denotes a re-registration of the key;
/// it fully supersedes any prior registration or bindings accumulated for that key.
/// This could happen when a single block deregisters and re-registers a credential.
impl<'a, K: Ord, L, R, V> DiffBind<&'a K, &'a L, &'a R, &'a V> {
    pub fn extend_refs<const REGISTERED: usize, const UNREGISTERED: usize>(
        &mut self,
        newer: &'a DiffBind<K, L, R, V, REGISTERED, UNREGISTERED>,
    ) -> &mut Self {
        for key in &newer.unregistered {
            self.unregister(key);
        }

        for (key, newer) in &newer.registered {
            self.unregistered.remove(key);

            match self.registered.entry(key) {
                Entry::Vacant(e) => {
                    e.insert(newer.as_refs());
                }

                Entry::Occupied(mut e) => {
                    e.get_mut().then(newer.as_refs());
                }
            };
        }

        self
    }
}

impl<K: Ord, L, R, V, const REGISTERED: usize, const UNREGISTERED: usize>
    DiffBind<K, L, R, V, REGISTERED, UNREGISTERED>
{
    /// Lookup the state of a Bind, if resolvable. `Existence::Unknown` means that we cannot
    /// conclude to anything without access to historical information.
    pub fn get(&self, k: &K) -> Existence<Bind<&L, &R, &V>> {
        if let Some(bind) = self.registered.get(k) {
            Existence::Exists(bind.as_refs())
        } else if self.unregistered.contains(k) {
            Existence::Gone
        } else {
            Existence::Unknown
        }
    }

    /// Return whether this diff contains no registrations, bindings, or removals.
    pub fn is_empty(&self) -> bool {
        self.registered.is_empty() && self.unregistered.is_empty()
    }

    /// Efficiently fold a borrowed sequence of `DiffBind` into a single aggregate.
    pub fn fold<'iter>(
        diffs: impl Iterator<Item = &'iter DiffBind<K, L, R, V, REGISTERED, UNREGISTERED>>,
    ) -> DiffBind<&'iter K, &'iter L, &'iter R, &'iter V>
    where
        K: 'iter,
        L: 'iter,
        R: 'iter,
        V: 'iter,
    {
        let mut fold = DiffBind::default();
        for diff in diffs {
            fold.extend_refs(diff);
        }
        fold
    }

    /// Register a key together with its value and optional left/right bindings.
    ///
    /// Returns an error if the key already has a pending registration in this diff.
    pub fn register(&mut self, key: K, value: V, left: Option<L>, right: Option<R>) -> Result<(), RegisterError<K>> {
        if self.registered.contains_key(&key) {
            return Err(RegisterError::AlreadyRegistered(key));
        }

        self.unregistered.remove(&key);
        self.registered
            .insert(key, Bind { left: Resettable::from(left), right: Resettable::from(right), value: Some(value) });

        Ok(())
    }

    /// Mark a key as unregistered, removing any pending registration or binding first.
    pub fn unregister(&mut self, key: K) {
        self.registered.remove(&key);
        self.unregistered.insert(key);
    }

    /// Update or create the pending left binding for a key.
    ///
    /// Returns an error if the key has already been marked as unregistered in this diff.
    pub fn bind_left(&mut self, key: K, left: Option<L>) -> Result<(), BindError<K>> {
        if self.unregistered.contains(&key) {
            return Err(BindError::AlreadyUnregistered(key));
        }

        match self.registered.entry(key) {
            Entry::Occupied(mut e) => {
                e.get_mut().left = Resettable::from(left);
            }
            Entry::Vacant(e) => {
                e.insert(Bind { left: Resettable::from(left), right: Resettable::Unchanged, value: None });
            }
        }

        Ok(())
    }

    /// Update or create the pending right binding for a key.
    ///
    /// Returns an error if the key has already been marked as unregistered in this diff.
    pub fn bind_right(&mut self, key: K, right: Option<R>) -> Result<(), BindError<K>> {
        if self.unregistered.contains(&key) {
            return Err(BindError::AlreadyUnregistered(key));
        }

        match self.registered.entry(key) {
            Entry::Occupied(mut e) => {
                e.get_mut().right = Resettable::from(right);
            }
            Entry::Vacant(e) => {
                e.insert(Bind { left: Resettable::Unchanged, right: Resettable::from(right), value: None });
            }
        }

        Ok(())
    }
}

impl<K: Ord, L, R, V> DiffBind<&K, &L, &R, &V> {
    /// Materialize a borrowed diff back into an owned one.
    pub fn owned(&self) -> DiffBind<K, L, R, V>
    where
        K: ToOwned<Owned = K>,
        L: ToOwned<Owned = L>,
        R: ToOwned<Owned = R>,
        V: ToOwned<Owned = V>,
    {
        DiffBind {
            unregistered: self.unregistered.iter().map(|k| (*k).to_owned()).collect(),
            registered: self.registered.iter().map(|(k, bind)| ((*k).to_owned(), bind.owned())).collect(),
        }
    }
}

/// Error returned when attempting to register the same key twice in a single diff.
#[derive(thiserror::Error, Debug)]
pub enum RegisterError<K> {
    /// The key already has a pending registration in this diff.
    #[error("key is already registered")]
    AlreadyRegistered(K),
}

/// Error returned when attempting to bind a key that has already been removed.
#[derive(thiserror::Error, Debug)]
pub enum BindError<K> {
    /// The key has already been marked as unregistered in this diff.
    #[error("key is already unregistered")]
    AlreadyUnregistered(K),
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::prelude::*;

    use super::DiffBind;
    #[cfg(test)]
    use crate::state::volatile::RegisterError;
    #[cfg(test)]
    use crate::state::volatile::{Bind, Existence, Resettable};

    #[derive(Debug)]
    enum Operation {
        Register(u8, u8, Option<u8>, Option<u8>),
        BindLeft(u8, Option<u8>),
        BindRight(u8, Option<u8>),
        Unregister(u8),
    }

    fn any_op() -> impl Strategy<Value = Operation> {
        let key = 0u8..8;
        prop_oneof![
            (key.clone(), any::<u8>(), prop::option::of(any::<u8>()), prop::option::of(any::<u8>()))
                .prop_map(|(k, v, l, r)| Operation::Register(k, v, l, r)),
            (key.clone(), prop::option::of(any::<u8>())).prop_map(|(k, l)| Operation::BindLeft(k, l)),
            (key.clone(), prop::option::of(any::<u8>())).prop_map(|(k, r)| Operation::BindRight(k, r)),
            key.prop_map(Operation::Unregister),
        ]
    }

    prop_compose! {
        /// An arbitrary [`DiffBind`] produced by replaying a sequence of registrations,
        /// bindings/unbindings and unregistrations over a small key space. The result is always
        /// a state reachable through the public API.
        #[expect(clippy::expect_used)]
        pub fn any_diff_bind()(
            ops in prop::collection::vec(any_op(), 0..24),
        ) -> DiffBind<u8, u8, u8, u8> {
            let mut diff = DiffBind::default();
            for op in ops {
                match op {
                    Operation::Register(k, v, l, r) if !diff.registered.contains_key(&k) => {
                        diff.register(k, v, l, r).expect("key not already registered");
                    }
                    Operation::BindLeft(k, l) if !diff.unregistered.contains(&k) => {
                        diff.bind_left(k, l).expect("key not unregistered");
                    }
                    Operation::BindRight(k, r) if !diff.unregistered.contains(&k) => {
                        diff.bind_right(k, r).expect("key not unregistered");
                    }
                    Operation::Unregister(k) => diff.unregister(k),
                    // Precondition not met: skip the mutation to keep the DiffBind consistent.
                    Operation::Register(..) | Operation::BindLeft(..) | Operation::BindRight(..) => {}
                }
            }
            diff
        }
    }

    #[test]
    fn register_some_left_then_bind_left() {
        let mut diff_bind = DiffBind::<_, _, _, _>::default();
        diff_bind.register(1, "value", Some("left_1"), None::<()>).unwrap();
        diff_bind.bind_left(1, Some("left_2")).unwrap();
        assert!(diff_bind.unregistered.is_empty());
        assert!(diff_bind.registered.contains_key(&1));
        assert_eq!(
            Some(&Bind { left: Resettable::Set("left_2"), right: Resettable::Reset, value: Some("value") }),
            diff_bind.registered.get(&1)
        );
    }

    #[test]
    fn register_some_left_then_bind_right() {
        let mut diff_bind = DiffBind::<_, _, _, _>::default();
        diff_bind.register(1, "value", None::<()>, Some("right_1")).unwrap();
        diff_bind.bind_right(1, Some("right_2")).unwrap();
        assert!(diff_bind.unregistered.is_empty());
        assert!(diff_bind.registered.contains_key(&1));
        assert_eq!(
            Some(&Bind { left: Resettable::Reset, right: Resettable::Set("right_2"), value: Some("value") }),
            diff_bind.registered.get(&1)
        );
    }

    #[test]
    fn register_some_left_then_unbind_left() {
        let mut diff_bind = DiffBind::<_, _, _, _>::default();
        diff_bind.register(1, "value", Some("left"), None::<()>).unwrap();
        diff_bind.bind_left(1, None).unwrap();
        assert!(diff_bind.unregistered.is_empty());
        assert!(diff_bind.registered.contains_key(&1));
        assert_eq!(
            Some(&Bind { left: Resettable::Reset, right: Resettable::Reset, value: Some("value") }),
            diff_bind.registered.get(&1)
        );
    }

    #[test]
    fn register_some_right_then_unbind_right() {
        let mut diff_bind = DiffBind::<_, _, _, _>::default();
        diff_bind.register(1, "value", None::<()>, Some("right")).unwrap();
        diff_bind.bind_right(1, None).unwrap();
        assert!(diff_bind.unregistered.is_empty());
        assert!(diff_bind.registered.contains_key(&1));
        assert_eq!(
            Some(&Bind { left: Resettable::Reset, right: Resettable::Reset, value: Some("value") }),
            diff_bind.registered.get(&1)
        );
    }

    #[test]
    fn register_then_unregister() {
        let mut diff_bind = DiffBind::<_, _, _, _>::default();
        diff_bind.register(1, "value", None::<()>, None::<()>).unwrap();
        diff_bind.unregister(1);
        assert!(diff_bind.unregistered.contains(&1));
        assert!(diff_bind.registered.is_empty());
    }

    #[test]
    fn register_none_then_bind_left() {
        let mut diff_bind = DiffBind::<_, _, _, _>::default();
        diff_bind.register(1, "value", None, None::<()>).unwrap();
        diff_bind.bind_left(1, Some("left")).unwrap();
        assert!(diff_bind.unregistered.is_empty());
        assert!(diff_bind.registered.contains_key(&1));
        assert_eq!(
            Some(&Bind { left: Resettable::Set("left"), right: Resettable::Reset, value: Some("value") }),
            diff_bind.registered.get(&1)
        );
    }

    #[test]
    fn register_none_then_bind_right() {
        let mut diff_bind = DiffBind::<_, _, _, _>::default();
        diff_bind.register(1, "value", None::<()>, None).unwrap();
        diff_bind.bind_right(1, Some("right")).unwrap();
        assert!(diff_bind.unregistered.is_empty());
        assert!(diff_bind.registered.contains_key(&1));
        assert_eq!(
            Some(&Bind { left: Resettable::Reset, right: Resettable::Set("right"), value: Some("value") }),
            diff_bind.registered.get(&1)
        );
    }

    #[test]
    fn register_none_then_bind_left_and_right() {
        let mut diff_bind = DiffBind::<_, _, _, _>::default();
        diff_bind.register(1, "value", None, None).unwrap();
        diff_bind.bind_left(1, Some("left")).unwrap();
        diff_bind.bind_right(1, Some("right")).unwrap();
        assert!(diff_bind.unregistered.is_empty());
        assert!(diff_bind.registered.contains_key(&1));
        assert_eq!(
            Some(&Bind { left: Resettable::Set("left"), right: Resettable::Set("right"), value: Some("value") }),
            diff_bind.registered.get(&1)
        );
    }

    #[test]
    fn bind_left_then_register_fails() {
        let mut diff_bind = DiffBind::<_, _, _, _>::default();
        diff_bind.bind_left(1, Some("left")).unwrap();
        assert!(matches!(
            diff_bind.register(1, "value", None, None::<()>),
            Err(RegisterError::AlreadyRegistered { .. })
        ));
    }

    #[test]
    fn bind_right_then_register_fails() {
        let mut diff_bind = DiffBind::<_, _, _, _>::default();
        diff_bind.bind_right(1, Some("right")).unwrap();
        assert!(matches!(
            diff_bind.register(1, "value", None::<()>, None),
            Err(RegisterError::AlreadyRegistered { .. })
        ));
    }

    #[test]
    fn bind_left_only() {
        let mut diff_bind = DiffBind::<_, _, _, _>::default();
        diff_bind.bind_left(1, Some("left")).unwrap();
        assert!(diff_bind.unregistered.is_empty());
        assert!(diff_bind.registered.contains_key(&1));
        assert_eq!(
            Some(&Bind { left: Resettable::Set("left"), right: Resettable::Unchanged::<()>, value: None::<()> }),
            diff_bind.registered.get(&1)
        );
    }

    #[test]
    fn bind_right_only() {
        let mut diff_bind = DiffBind::<_, _, _, _>::default();
        diff_bind.bind_right(1, Some("right")).unwrap();
        assert!(diff_bind.unregistered.is_empty());
        assert!(diff_bind.registered.contains_key(&1));
        assert_eq!(
            Some(&Bind { left: Resettable::Unchanged::<()>, right: Resettable::Set("right"), value: None::<()> }),
            diff_bind.registered.get(&1)
        );
    }

    #[test]
    fn extend_reregistration_supersedes_prior_binding() {
        let key = 1;
        let right = "abstain".to_string();

        // Accumulated window state: key 1 was only re-bound (e.g. a pure vote delegation), not
        // registered within the window: { left: Unchanged, right: Set, value: None }.
        let mut current = DiffBind::<_, _, _, _>::default();
        current.bind_right(&key, Some(&right)).unwrap();

        // A later fragment deregisters then re-registers key 1 within a single block, which
        // collapses to a plain registration: { left: Reset, right: Reset, value: Some }.
        let mut next = DiffBind::<_, _, _, _>::default();
        next.register(1, 42, None::<String>, None).unwrap();

        current.extend_refs(&next);

        assert!(current.unregistered.is_empty());
        assert_eq!(
            Some(&Bind { left: Resettable::Reset, right: Resettable::Reset, value: Some(&42) }),
            current.registered.get(&key)
        );
    }

    #[test]
    fn extend_binding_update_preserves_existing_registration() {
        let key = 1;
        let value = 42;
        let right = "abstain".to_string();

        let mut current = DiffBind::<_, _, _, _>::default();
        current.register(&key, &value, None::<&String>, None::<&String>).unwrap();

        // A later fragment only re-binds the right: the existing deposit and
        // the untouched left must be preserved.
        let mut next = DiffBind::<_, _, _, _>::default();
        next.bind_right(1, Some(right.clone())).unwrap();

        current.extend_refs(&next);

        assert_eq!(
            Some(&Bind { left: Resettable::Reset, right: Resettable::Set(&right), value: Some(&value) }),
            current.registered.get(&1)
        );
    }

    #[test]
    fn is_empty_reflects_contents() {
        let mut diff = DiffBind::<u8, (), (), &str>::default();
        assert!(diff.is_empty());

        diff.register(1, "value", None, None).unwrap();
        assert!(!diff.is_empty());

        diff.unregister(1);
        assert!(!diff.is_empty());
    }

    #[test]
    fn get_resolves_existence() {
        let mut diff = DiffBind::<u8, u8, u8, u8>::default();
        diff.register(1, 100, Some(10), None).unwrap();
        diff.bind_left(2, Some(10)).unwrap(); // bind-only: registered without a value
        diff.unregister(3);

        match diff.get(&1) {
            Existence::Exists(bind) => {
                assert_eq!(bind.value, Some(&100));
                assert!(matches!(bind.left, Resettable::Set(&10)));
            }
            other @ (Existence::Gone | Existence::Unknown) => panic!("expected Exists, got {other:?}"),
        }

        match diff.get(&2) {
            Existence::Exists(bind) => assert_eq!(bind.value, None),
            other @ (Existence::Gone | Existence::Unknown) => panic!("expected bind-only Exists, got {other:?}"),
        }

        assert!(matches!(diff.get(&3), Existence::Gone));
        assert!(matches!(diff.get(&4), Existence::Unknown));
    }

    #[test]
    fn fold_empty_is_default() {
        let folded = DiffBind::fold(std::iter::empty::<&DiffBind<u8, u8, u8, u8>>());
        assert_eq!(folded, DiffBind::default());
    }

    proptest! {
        /// Folding a borrowed sequence must equal applying each diff in order via `extend`. This is
        /// the property the volatile aggregate relies on to resolve an account by folding its
        /// windowed per-fragment contributions on read.
        #[test]
        fn fold_matches_sequential_extend(diffs in prop::collection::vec(any_diff_bind(), 0..6)) {
            let folded = DiffBind::fold(diffs.iter());

            let sequential = diffs.iter().fold(DiffBind::default(), |mut acc, diff| {
                acc.extend_refs(diff);
                acc
            });

            prop_assert_eq!(folded, sequential);
        }
    }
}
