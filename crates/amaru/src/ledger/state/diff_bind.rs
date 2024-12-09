use std::collections::{btree_map::Entry, BTreeMap, BTreeSet};

/// A compact data-structure tracking changes in a DAG which supports optional linking of values with
/// another data-structure. Items can only be linked if they have been registered first. Yet, they
/// can be unlinked without being unregistered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffBind<K: Ord, J, V> {
    pub registered: BTreeMap<K, (Option<J>, Option<V>)>,
    pub unregistered: BTreeSet<K>,
}

impl<K: Ord, J, V> Default for DiffBind<K, J, V> {
    fn default() -> Self {
        Self {
            registered: Default::default(),
            unregistered: Default::default(),
        }
    }
}

impl<K: Ord + std::fmt::Debug, J: Clone + std::fmt::Debug, V> DiffBind<K, J, V> {
    pub fn register(&mut self, k: K, v: V, j: Option<J>) {
        self.unregistered.remove(&k);
        self.registered.insert(k, (j, Some(v)));
    }

    pub fn bind(&mut self, k: K, j: Option<J>) {
        assert!(!self.unregistered.contains(&k));
        match self.registered.entry(k) {
            Entry::Occupied(mut e) => {
                e.get_mut().0 = j;
            }
            Entry::Vacant(e) => {
                e.insert((j, None));
            }
        }
    }

    pub fn unregister(&mut self, k: K) {
        self.registered.remove(&k);
        self.unregistered.insert(k);
    }
}
