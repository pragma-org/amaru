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

use std::{
    collections::{BTreeMap, BTreeSet, btree_map::Entry},
    mem,
};

use crate::state::volatile::Existence;

/// A compact data-structure tracking changes in a DAG which supports optional linking of values with
/// another data-structure. Items can only be linked if they have been registered first. Yet, they
/// can be unlinked without being unregistered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffBind<K: Ord, L, R, V> {
    pub registered: BTreeMap<K, Bind<L, R, V>>,
    pub unregistered: BTreeSet<K>,
}

impl<K: Ord, L, R, V> DiffBind<K, L, R, V> {
    pub fn is_empty(&self) -> bool {
        self.registered.is_empty() && self.unregistered.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bind<L, R, V> {
    pub left: Resettable<L>,
    pub right: Resettable<R>,
    pub value: Option<V>,
}

impl<L, R, V> Default for Bind<L, R, V> {
    fn default() -> Self {
        Self { left: Resettable::default(), right: Resettable::default(), value: None }
    }
}

impl<L, R, V> Bind<L, R, V> {
    pub fn as_borrowed(&self) -> Bind<&L, &R, &V> {
        Bind { left: self.left.as_borrowed(), right: self.right.as_borrowed(), value: self.value.as_ref() }
    }

    /// Absorb a more recent update in place.
    /// A `Set`/`Reset` overrides, `Unchanged` keeps what's here,
    /// and a `value: Some(...)` supersedes wholesale.
    pub fn then(&mut self, newer: Self) {
        if newer.value.is_some() {
            *self = newer;
        } else {
            if !matches!(newer.left, Resettable::Unchanged) {
                self.left = newer.left;
            }
            if !matches!(newer.right, Resettable::Unchanged) {
                self.right = newer.right;
            }
        }
    }
}

impl<L: ToOwned<Owned = L>, R: ToOwned<Owned = R>, V: ToOwned<Owned = V>> Bind<&L, &R, &V> {
    pub fn to_owned(&self) -> Bind<L, R, V> {
        Bind { left: self.left.to_owned(), right: self.right.to_owned(), value: self.value.map(|v| v.to_owned()) }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Resettable<A> {
    Set(A),
    Reset,
    #[default]
    Unchanged,
}

impl<A> Resettable<A> {
    /// Apply this change to `value`, returning the previous content when a change occurred.
    ///
    /// - `Unchanged` => returns `None` and leaves `value` as-is
    /// - `Set(new)`  => replaces `value` with `Some(new)` and returns the old `Option<A>`
    /// - `Reset`     => sets `value` to `None` and returns the old `Option<A>`
    pub fn set_or_reset(self, value: &mut Option<A>) -> Option<A> {
        match self {
            Resettable::Unchanged => None,
            Resettable::Set(new) => Option::replace(value, new),
            Resettable::Reset => mem::take(value),
        }
    }

    pub fn as_borrowed(&self) -> Resettable<&A> {
        match self {
            Self::Set(value) => Resettable::Set(value),
            Self::Reset => Resettable::Reset,
            Self::Unchanged => Resettable::Unchanged,
        }
    }

    /// Transform into an `Option`, using the default value `when_unchanged` for the `Unchanged`
    /// case.
    pub fn into_option(self, when_unchanged: Option<A>) -> Option<A> {
        match self {
            Resettable::Set(value) => Some(value),
            Resettable::Reset => None,
            Resettable::Unchanged => when_unchanged,
        }
    }
}

impl<A: ToOwned<Owned = A>> Resettable<&A> {
    pub fn to_owned(&self) -> Resettable<A> {
        match self {
            Self::Set(a) => Resettable::Set((*a).to_owned()),
            Self::Reset => Resettable::Reset,
            Self::Unchanged => Resettable::Unchanged,
        }
    }

    /// Transform into an `Option`, using the default value `when_unchanged` for the `Unchanged`
    /// case.
    pub fn to_option(&self, when_unchanged: Option<&A>) -> Option<A> {
        match self {
            Resettable::Set(value) => Some((*value).to_owned()),
            Resettable::Reset => None,
            Resettable::Unchanged => when_unchanged.map(|value| value.to_owned()),
        }
    }
}

impl<A> From<Option<A>> for Resettable<A> {
    fn from(opt: Option<A>) -> Self {
        match opt {
            None => Resettable::Reset,
            Some(r) => Resettable::Set(r),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Empty;

impl<K: Ord, L, R, V> Default for DiffBind<K, L, R, V> {
    fn default() -> Self {
        Self { registered: Default::default(), unregistered: Default::default() }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum RegisterError<K> {
    #[error("key is already registered")]
    AlreadyRegistered(K),
}

#[derive(thiserror::Error, Debug)]
pub enum BindError<K> {
    #[error("key is already unregistered")]
    AlreadyUnregistered(K),
}

impl<K: Ord, L, R, V> DiffBind<K, L, R, V> {
    pub fn as_borrowed(&self) -> DiffBind<&K, &L, &R, &V> {
        DiffBind {
            unregistered: self.unregistered.iter().collect(),
            registered: self.registered.iter().map(|(k, bind)| (k, bind.as_borrowed())).collect(),
        }
    }

    /// Lookup the state of a Bind, if resolvable. `Existence::Unknown` means that we cannot
    /// conclude to anything without access to historical information.
    pub fn lookup(&self, k: &K) -> Existence<Bind<&L, &R, &V>>
    where
        L: ToOwned<Owned = L>,
        R: ToOwned<Owned = R>,
        V: ToOwned<Owned = V>,
    {
        if let Some(bind) = self.registered.get(k) {
            Existence::Exists(bind.as_borrowed())
        } else if self.unregistered.contains(k) {
            Existence::Gone
        } else {
            Existence::Unknown
        }
    }

    /// Efficiently fold a borrowed sequence of `DiffBind` into a single aggregate.
    pub fn fold<'iter>(
        diffs: impl Iterator<Item = &'iter DiffBind<K, L, R, V>>,
    ) -> DiffBind<&'iter K, &'iter L, &'iter R, &'iter V>
    where
        K: 'iter,
        L: 'iter,
        R: 'iter,
        V: 'iter,
    {
        let mut fold = DiffBind::default();
        for diff in diffs {
            fold.append(diff.as_borrowed());
        }
        fold
    }

    /// Merge two states together, assuming that the other is a more recent update.
    ///
    /// Importantly, this composes two already-validated `DiffBind`s, it does not re-validate them.
    ///
    /// In particular, a `value: Some(...)` in `most_recent` denotes a re-registration of the key;
    /// it fully supersedes any prior registration or bindings accumulated for that key.
    /// This could happen when a single block deregisters and re-registers a credential.
    pub fn append(&mut self, newer: Self) -> &mut Self {
        for key in newer.unregistered {
            self.unregister(key);
        }

        for (key, newer) in newer.registered {
            self.unregistered.remove(&key);

            match self.registered.entry(key) {
                Entry::Vacant(e) => {
                    e.insert(newer);
                }

                Entry::Occupied(mut e) => {
                    e.get_mut().then(newer);
                }
            };
        }

        self
    }

    pub fn register(&mut self, key: K, value: V, left: Option<L>, right: Option<R>) -> Result<(), RegisterError<K>> {
        if self.registered.contains_key(&key) {
            return Err(RegisterError::AlreadyRegistered(key));
        }

        self.unregistered.remove(&key);
        self.registered
            .insert(key, Bind { left: Resettable::from(left), right: Resettable::from(right), value: Some(value) });

        Ok(())
    }

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

    pub fn unregister(&mut self, key: K) {
        self.registered.remove(&key);
        self.unregistered.insert(key);
    }
}

impl<K, L, R, V> DiffBind<&K, &L, &R, &V>
where
    K: Ord + ToOwned<Owned = K>,
    L: ToOwned<Owned = L>,
    R: ToOwned<Owned = R>,
    V: ToOwned<Owned = V>,
{
    pub fn to_owned(&self) -> DiffBind<K, L, R, V> {
        DiffBind {
            unregistered: self.unregistered.iter().map(|k| (*k).to_owned()).collect(),
            registered: self.registered.iter().map(|(k, bind)| ((*k).to_owned(), bind.to_owned())).collect(),
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use proptest::prelude::*;

    use super::DiffBind;

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
        pub(crate) fn arbitrary_diff_bind()(
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
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::{test_support::arbitrary_diff_bind, *};
    use crate::state::volatile::Existence;

    #[test]
    fn register_some_left_then_bind_left() {
        let mut diff_bind = DiffBind::default();
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
        let mut diff_bind = DiffBind::default();
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
        let mut diff_bind = DiffBind::default();
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
        let mut diff_bind = DiffBind::default();
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
        let mut diff_bind = DiffBind::default();
        diff_bind.register(1, "value", None::<()>, None::<()>).unwrap();
        diff_bind.unregister(1);
        assert!(diff_bind.unregistered.contains(&1));
        assert!(diff_bind.registered.is_empty());
    }

    #[test]
    fn register_none_then_bind_left() {
        let mut diff_bind = DiffBind::default();
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
        let mut diff_bind = DiffBind::default();
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
        let mut diff_bind = DiffBind::default();
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
        let mut diff_bind = DiffBind::default();
        diff_bind.bind_left(1, Some("left")).unwrap();
        assert!(matches!(
            diff_bind.register(1, "value", None, None::<()>),
            Err(RegisterError::AlreadyRegistered { .. })
        ));
    }

    #[test]
    fn bind_right_then_register_fails() {
        let mut diff_bind = DiffBind::default();
        diff_bind.bind_right(1, Some("right")).unwrap();
        assert!(matches!(
            diff_bind.register(1, "value", None::<()>, None),
            Err(RegisterError::AlreadyRegistered { .. })
        ));
    }

    #[test]
    fn bind_left_only() {
        let mut diff_bind = DiffBind::default();
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
        let mut diff_bind = DiffBind::default();
        diff_bind.bind_right(1, Some("right")).unwrap();
        assert!(diff_bind.unregistered.is_empty());
        assert!(diff_bind.registered.contains_key(&1));
        assert_eq!(
            Some(&Bind { left: Resettable::Unchanged::<()>, right: Resettable::Set("right"), value: None::<()> }),
            diff_bind.registered.get(&1)
        );
    }

    #[test]
    fn append_reregistration_supersedes_prior_binding() {
        // Accumulated window state: key 1 was only re-bound (e.g. a pure vote delegation), not
        // registered within the window: { left: Unchanged, right: Set, value: None }.
        let mut current = DiffBind::default();
        current.bind_right(1, Some("abstain")).unwrap();

        // A later fragment deregisters then re-registers key 1 within a single block, which
        // collapses to a plain registration: { left: Reset, right: Reset, value: Some }.
        let mut next = DiffBind::default();
        next.register(1, "deposit", None::<&str>, None).unwrap();

        current.append(next);

        assert!(current.unregistered.is_empty());
        assert_eq!(
            Some(&Bind { left: Resettable::Reset, right: Resettable::Reset, value: Some("deposit") }),
            current.registered.get(&1)
        );
    }

    #[test]
    fn append_binding_update_preserves_existing_registration() {
        let mut current = DiffBind::default();
        current.register(1, "deposit", None::<&str>, None::<&str>).unwrap();

        // A later fragment only re-binds the right: the existing deposit and
        // the untouched left must be preserved.
        let mut next = DiffBind::default();
        next.bind_right(1, Some("abstain")).unwrap();

        current.append(next);

        assert_eq!(
            Some(&Bind { left: Resettable::Reset, right: Resettable::Set("abstain"), value: Some("deposit") }),
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
    fn lookup_resolves_existence() {
        let mut diff = DiffBind::<u8, u8, u8, u8>::default();
        diff.register(1, 100, Some(10), None).unwrap();
        diff.bind_left(2, Some(10)).unwrap(); // bind-only: registered without a value
        diff.unregister(3);

        match diff.lookup(&1) {
            Existence::Exists(bind) => {
                assert_eq!(bind.value, Some(&100));
                assert!(matches!(bind.left, Resettable::Set(&10)));
            }
            other @ (Existence::Gone | Existence::Unknown) => panic!("expected Exists, got {other:?}"),
        }

        match diff.lookup(&2) {
            Existence::Exists(bind) => assert_eq!(bind.value, None),
            other @ (Existence::Gone | Existence::Unknown) => panic!("expected bind-only Exists, got {other:?}"),
        }

        assert!(matches!(diff.lookup(&3), Existence::Gone));
        assert!(matches!(diff.lookup(&4), Existence::Unknown));
    }

    #[test]
    fn fold_empty_is_default() {
        let folded = DiffBind::fold(std::iter::empty::<&DiffBind<u8, u8, u8, u8>>()).to_owned();
        assert_eq!(folded, DiffBind::default());
    }

    proptest! {
        /// Folding a borrowed sequence must equal applying each diff in order via `append`. This is
        /// the property `VolatileSeries::resolve_account` relies on when it recomputes the accounts
        /// aggregate lazily from the fragments.
        #[test]
        fn fold_matches_sequential_append(diffs in prop::collection::vec(arbitrary_diff_bind(), 0..6)) {
            let folded = DiffBind::fold(diffs.iter()).to_owned();

            let sequential = diffs.iter().fold(DiffBind::default(), |mut acc, diff| {
                acc.append(diff.clone());
                acc
            });

            prop_assert_eq!(folded, sequential);
        }
    }
}
