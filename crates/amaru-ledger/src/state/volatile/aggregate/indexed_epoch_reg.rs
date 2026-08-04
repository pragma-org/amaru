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

use std::collections::BTreeMap;

use crate::state::volatile::DiffEpochReg;

/// The window's pools, counted by id: how many fragments in the window registered each one. Pool
/// existence is monotonic-additive here; `register` adds a pool, and an `unregister` is a deferred
/// retirement resolved at the epoch boundary (by the overlay), never in this aggregate. Retracting
/// the oldest fragment (stabilization) and the newest (rollback) are therefore the same decrement.
#[derive(Debug, Clone)]
pub struct IndexedEpochReg<K: Ord> {
    index: BTreeMap<K, usize>,
}

impl<K: Ord> Default for IndexedEpochReg<K> {
    fn default() -> Self {
        Self { index: BTreeMap::default() }
    }
}

impl<K: Ord> IndexedEpochReg<K> {
    /// Whether any fragment in the window registered this entity.
    pub fn get(&self, key: &K) -> bool {
        self.index.contains_key(key)
    }

    /// Record a fragment's registrations, one increment per entity seen registered or unregistered.
    pub fn extend<V>(&mut self, diff: &DiffEpochReg<K, V>)
    where
        K: Copy,
    {
        for key in diff.registered.keys().chain(diff.unregistered.keys()) {
            *self.index.entry(*key).or_default() += 1;
        }
    }

    /// Remove a fragment's, one decrement per entity it references, dropping a it
    /// once no fragment in the window references it.
    pub fn remove<V>(&mut self, diff: &DiffEpochReg<K, V>) -> bool {
        let mut all_present = true;

        for key in diff.registered.keys().chain(diff.unregistered.keys()) {
            match self.index.get_mut(key) {
                Some(count) => {
                    *count -= 1;
                    if *count == 0 {
                        self.index.remove(key);
                    }
                }
                None => all_present = false,
            }
        }

        all_present
    }
}

#[cfg(test)]
mod tests {
    use super::IndexedEpochReg;
    use crate::state::volatile::DiffEpochReg;

    fn registers(tag: u8) -> DiffEpochReg<u8, ()> {
        let mut diff = DiffEpochReg::default();
        diff.register(tag, ());
        diff
    }

    #[test]
    fn entry_stays_registered_until_every_registering_fragment_is_retracted() {
        let mut index = IndexedEpochReg::default();

        index.extend(&registers(1));
        assert!(index.get(&1));
        index.extend(&registers(1));
        assert!(index.get(&1));

        assert!(index.remove(&registers(1)));
        assert!(index.get(&1), "a second registration keeps the entry live");

        assert!(index.remove(&registers(1)));
        assert!(!index.get(&1), "retracting the last registration drops the entry");

        assert!(!index.remove(&registers(1)), "removing non-existing element should return false");
    }
}
