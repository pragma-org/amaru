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

use crate::context::Delta;

/// A compact data-structure tracking changes in a DAG which supports optional linking of values with
/// another data-structure. Items can only be linked if they have been registered first. Yet, they
/// can be unlinked without being unregistered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffBind<K: Ord, L, R, V> {
    pub registered: BTreeMap<K, Bind<L, R, V>>,
    pub unregistered: BTreeSet<K>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bind<L, R, V> {
    pub left: Resettable<L>,
    pub right: Resettable<R>,
    pub value: Option<V>,
}

impl<L, R, V> Bind<L, R, V> {
    pub fn into_borrowed(&self) -> Bind<&L, &R, &V> {
        Bind { left: self.left.into_borrowed(), right: self.right.into_borrowed(), value: self.value.as_ref() }
    }
}

impl<L: ToOwned<Owned = L>, R: ToOwned<Owned = R>, V: ToOwned<Owned = V>> Bind<&L, &R, &V> {
    pub fn to_owned(&self) -> Bind<L, R, V> {
        Bind { left: self.left.to_owned(), right: self.right.to_owned(), value: self.value.map(|v| v.to_owned()) }
    }
}

/// The materialized counterpart of [`Bind`]: a registered entry holding a value and its optional
/// left/right links.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bound<L, R, V> {
    pub left: Option<L>,
    pub right: Option<R>,
    pub value: V,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resettable<A> {
    Set(A),
    Reset,
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

    /// Apply this change to `slot` in place, returning the change that undoes it.
    pub fn apply_to(self, slot: &mut Option<A>) -> Resettable<A> {
        match self {
            Resettable::Unchanged => Resettable::Unchanged,
            Resettable::Set(new) => Resettable::from(Option::replace(slot, new)),
            Resettable::Reset => Resettable::from(mem::take(slot)),
        }
    }

    /// Materialize this change as the absolute value of a fresh entry: `Set` becomes `Some`, while
    /// `Reset` and `Unchanged` both become `None`.
    pub fn into_option(self) -> Option<A> {
        match self {
            Resettable::Set(a) => Some(a),
            Resettable::Reset | Resettable::Unchanged => None,
        }
    }

    pub fn into_borrowed(&self) -> Resettable<&A> {
        match self {
            Self::Set(a) => Resettable::Set(a),
            Self::Reset => Resettable::Reset,
            Self::Unchanged => Resettable::Unchanged,
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
}

impl<A> From<Option<A>> for Resettable<A> {
    fn from(opt: Option<A>) -> Self {
        match opt {
            None => Resettable::Reset,
            Some(r) => Resettable::Set(r),
        }
    }
}

#[derive(Debug)]
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
pub enum MergeError<K> {
    #[error("key is already registered")]
    AlreadyRegistered(K),
}

impl<K: ToOwned<Owned = K>> MergeError<&K> {
    pub fn to_owned(self) -> MergeError<K> {
        let Self::AlreadyRegistered(k) = self;
        MergeError::AlreadyRegistered(k.to_owned())
    }
}

#[derive(thiserror::Error, Debug)]
pub enum BindError<K> {
    #[error("key is already unregistered")]
    AlreadyUnregistered(K),
}

impl<K: Ord, L, R, V> DiffBind<K, L, R, V> {
    pub fn into_borrowed(&self) -> DiffBind<&K, &L, &R, &V> {
        DiffBind {
            unregistered: self.unregistered.iter().collect(),
            registered: self.registered.iter().map(|(k, bind)| (k, bind.into_borrowed())).collect(),
        }
    }

    /// Merge two states together, assuming that the other is a more recent update.
    pub fn append(&mut self, most_recent: Self) -> Result<&mut Self, MergeError<K>> {
        for key in most_recent.unregistered {
            self.unregister(key);
        }

        for (key, bind) in most_recent.registered {
            if self.registered.contains_key(&key) && bind.value.is_some() {
                return Err(MergeError::AlreadyRegistered(key));
            }

            self.unregistered.remove(&key);

            match self.registered.entry(key) {
                Entry::Vacant(e) => {
                    e.insert(bind);
                }

                Entry::Occupied(mut e) => {
                    if !matches!(&bind.left, &Resettable::Unchanged) {
                        e.get_mut().left = bind.left;
                    }

                    if !matches!(&bind.right, &Resettable::Unchanged) {
                        e.get_mut().right = bind.right;
                    }
                }
            };
        }

        Ok(self)
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

impl<K, L, R, V> Delta for DiffBind<K, L, R, V>
where
    K: Ord + Clone,
    L: Clone,
    R: Clone,
    V: Clone,
{
    type State = BTreeMap<K, Bound<L, R, V>>;
    type Error = MergeError<K>;

    fn apply(&self, base: &mut Self::State) -> Self {
        let mut undo = DiffBind::default();

        for key in &self.unregistered {
            if let Some(prev) = base.remove(key) {
                undo.registered.insert(
                    key.clone(),
                    Bind {
                        left: Resettable::from(prev.left),
                        right: Resettable::from(prev.right),
                        value: Some(prev.value),
                    },
                );
            }
        }

        for (key, bind) in &self.registered {
            match base.get_mut(key) {
                Some(entry) => {
                    let left = bind.left.clone().apply_to(&mut entry.left);
                    let right = bind.right.clone().apply_to(&mut entry.right);
                    let value = bind.value.clone().map(|v| mem::replace(&mut entry.value, v));
                    undo.registered.insert(key.clone(), Bind { left, right, value });
                }
                None => {
                    #[expect(clippy::expect_used)]
                    let value = bind.value.clone().expect("registered entry must carry a value");
                    base.insert(
                        key.clone(),
                        Bound { left: bind.left.clone().into_option(), right: bind.right.clone().into_option(), value },
                    );
                    undo.unregistered.insert(key.clone());
                }
            }
        }

        undo
    }

    fn compose(&mut self, next: &Self) -> Result<(), Self::Error> {
        self.append(next.clone())?;
        Ok(())
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
mod tests {
    use super::*;
    use crate::context::Delta;

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
    fn apply_then_undo_restores_base() {
        let mut base = BTreeMap::from([
            (1, Bound { left: Some("pool_a"), right: None, value: 100 }),
            (2, Bound { left: None, right: Some("drep_x"), value: 200 }),
        ]);
        let original = base.clone();

        let mut diff = DiffBind::default();
        diff.register(3, 300, Some("pool_c"), None::<&str>).unwrap();
        diff.bind_left(1, Some("pool_b")).unwrap();
        diff.unregister(2);

        let undo = diff.apply(&mut base);
        undo.apply(&mut base);

        assert_eq!(base, original);
    }

    #[test]
    fn compose_matches_sequential_apply() {
        let mut composed_base = BTreeMap::from([(1, Bound { left: Some("a"), right: None::<&str>, value: 10 })]);
        let mut sequential_base = composed_base.clone();

        let mut first = DiffBind::default();
        first.bind_left(1, Some("b")).unwrap();
        let mut second = DiffBind::default();
        second.register(2, 20, None::<&str>, None::<&str>).unwrap();

        first.apply(&mut sequential_base);
        second.apply(&mut sequential_base);

        first.compose(&second).unwrap();
        first.apply(&mut composed_base);

        assert_eq!(composed_base, sequential_base);
    }
}
