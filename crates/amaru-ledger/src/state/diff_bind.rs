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

impl<K: Ord, J: Clone, V> DiffBind<K, J, V> {
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
