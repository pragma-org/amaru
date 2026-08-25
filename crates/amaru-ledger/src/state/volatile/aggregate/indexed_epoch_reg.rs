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

use amaru_kernel::{Hash, PoolParams, size::VRF_KEY};

use crate::state::volatile::DiffEpochReg;

// --------------------------------------------------------------------------------- IndexedEpochReg

/// The window's pools, counted by id: how many fragments in the window registered each one. Pool
/// existence is monotonic-additive here; `register` adds a pool, and an `unregister` is a deferred
/// retirement resolved at the epoch boundary (by the overlay), never in this aggregate. Retracting
/// the oldest fragment (stabilization) and the newest (rollback) are therefore the same decrement.
#[derive(Debug, Clone)]
pub struct IndexedEpochReg<K: Ord> {
    index: BTreeMap<K, PoolCertificateCounters>,
}

impl<K: Ord> Default for IndexedEpochReg<K> {
    fn default() -> Self {
        Self { index: BTreeMap::default() }
    }
}

impl<K: Ord> IndexedEpochReg<K> {
    /// Whether any fragment in the window registered this entity.
    pub fn get(&self, key: &K) -> Option<&PoolCertificateCounters> {
        self.index.get(key)
    }

    /// Record a fragment's registrations, one increment per entity seen registered or unregistered.
    pub fn extend<V: HasVrf>(&mut self, diff: &DiffEpochReg<K, V>)
    where
        K: Copy,
    {
        for (key, registration) in &diff.registered {
            self.index.entry(*key).or_default().increment_registrations(*registration.last().vrf());
        }

        for key in diff.unregistered.keys() {
            self.index.entry(*key).or_default().increment_deregistrations();
        }
    }

    /// Remove a fragment's, one decrement per entity it references, dropping a it
    /// once no fragment in the window references it.
    pub fn remove<V: HasVrf>(&mut self, diff: &DiffEpochReg<K, V>) -> bool {
        let mut all_present = true;

        for key in diff.registered.keys() {
            match self.index.get_mut(key) {
                Some(count) => {
                    if count.decrement_registrations() {
                        self.index.remove(key);
                    }
                }
                None => all_present = false,
            }
        }

        for key in diff.unregistered.keys() {
            match self.index.get_mut(key) {
                Some(count) => {
                    if count.decrement_deregistrations() {
                        self.index.remove(key);
                    }
                }
                None => all_present = false,
            }
        }

        all_present
    }
}

// ------------------------------------------------------------------------- HasVrf

pub trait HasVrf {
    fn vrf(&self) -> &Hash<VRF_KEY>;
}

impl HasVrf for PoolParams {
    fn vrf(&self) -> &Hash<VRF_KEY> {
        &self.vrf
    }
}

// ------------------------------------------------------------------------- PoolCertificateCounters

/// A type counting pool registrations/deregistrations and retaining the first VRF key hash that is
/// seen. The latter is useful when resolving pool registrations that are only present in the
/// volatile DB and for which, we must know the active VRF.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PoolCertificateCounters {
    #[default]
    Empty,
    OnlyRegistrations {
        first_known_vrf: Hash<VRF_KEY>,
        registrations: usize,
    },
    OnlyDeregistrations {
        deregistrations: usize,
    },
    Both {
        first_known_vrf: Hash<VRF_KEY>,
        registrations: usize,
        deregistrations: usize,
    },
}

impl PoolCertificateCounters {
    pub fn is_empty(&self) -> bool {
        matches!(self, Self::Empty)
    }

    pub fn registrations(&self) -> usize {
        match self {
            Self::Empty | Self::OnlyDeregistrations { .. } => 0,
            Self::Both { registrations, .. } | Self::OnlyRegistrations { registrations, .. } => *registrations,
        }
    }

    pub fn deregistrations(&self) -> usize {
        match self {
            Self::Empty | Self::OnlyRegistrations { .. } => 0,
            Self::Both { deregistrations, .. } | Self::OnlyDeregistrations { deregistrations } => *deregistrations,
        }
    }

    pub fn first_known_vrf(&self) -> Option<&Hash<VRF_KEY>> {
        match self {
            Self::Empty | Self::OnlyDeregistrations { .. } => None,
            Self::Both { first_known_vrf, .. } | Self::OnlyRegistrations { first_known_vrf, .. } => {
                Some(first_known_vrf)
            }
        }
    }

    /// Increment the registrations counter, keeping the VRF if this is the first registration seen.
    fn increment_registrations(&mut self, vrf: Hash<VRF_KEY>) {
        *self = match self {
            Self::Empty => Self::OnlyRegistrations { first_known_vrf: vrf, registrations: 1 },
            Self::OnlyDeregistrations { deregistrations } => {
                Self::Both { first_known_vrf: vrf, registrations: 1, deregistrations: *deregistrations }
            }
            Self::OnlyRegistrations { registrations, .. } | Self::Both { registrations, .. } => {
                *registrations += 1;
                *self
            }
        };
    }

    /// Decrement the registration counters, returning true if the counters are now all zeroes/empty.
    fn decrement_registrations(&mut self) -> bool {
        *self = match self {
            Self::Empty | Self::OnlyDeregistrations { .. } => *self,
            Self::OnlyRegistrations { registrations, .. } => {
                if *registrations > 1 {
                    *registrations -= 1;
                    *self
                } else {
                    Self::Empty
                }
            }
            Self::Both { registrations, deregistrations, .. } => {
                if *registrations > 1 {
                    *registrations -= 1;
                    *self
                } else {
                    Self::OnlyDeregistrations { deregistrations: *deregistrations }
                }
            }
        };

        self.is_empty()
    }

    /// Increment the de-registrations counter
    fn increment_deregistrations(&mut self) {
        *self = match self {
            Self::Empty => Self::OnlyDeregistrations { deregistrations: 1 },
            Self::OnlyDeregistrations { deregistrations } | Self::Both { deregistrations, .. } => {
                *deregistrations += 1;
                *self
            }
            Self::OnlyRegistrations { first_known_vrf, registrations } => {
                Self::Both { first_known_vrf: *first_known_vrf, registrations: *registrations, deregistrations: 1 }
            }
        }
    }

    /// Decrement the registration counters, returning true if the counters are now all zeroes/empty.
    fn decrement_deregistrations(&mut self) -> bool {
        *self = match self {
            Self::Empty | Self::OnlyRegistrations { .. } => *self,
            Self::OnlyDeregistrations { deregistrations } => {
                if *deregistrations > 1 {
                    *deregistrations -= 1;
                    *self
                } else {
                    Self::Empty
                }
            }
            Self::Both { first_known_vrf, registrations, deregistrations } => {
                if *deregistrations > 1 {
                    *deregistrations -= 1;
                    *self
                } else {
                    Self::OnlyRegistrations { first_known_vrf: *first_known_vrf, registrations: *registrations }
                }
            }
        };

        self.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use amaru_kernel::{PoolParams, any_pool_params, utils::tests::run_strategy};

    use super::IndexedEpochReg;
    use crate::state::volatile::DiffEpochReg;

    fn registers(tag: u8) -> DiffEpochReg<u8, PoolParams> {
        let mut diff = DiffEpochReg::default();
        diff.register(tag, run_strategy(any_pool_params()));
        diff
    }

    fn unregisters(tag: u8) -> DiffEpochReg<u8, PoolParams> {
        let mut diff = DiffEpochReg::default();
        diff.unregister(tag, Default::default());
        diff
    }

    #[test]
    fn entry_stays_registered_until_every_registering_fragment_is_retracted() {
        let mut index = IndexedEpochReg::default();

        index.extend(&registers(1));
        let elem1 = index.get(&1).copied().unwrap();
        assert_eq!(elem1.registrations(), 1);
        assert_eq!(elem1.deregistrations(), 0);

        index.extend(&unregisters(1));
        let elem2 = index.get(&1).copied().unwrap();
        assert_eq!(elem2.registrations(), 1);
        assert_eq!(elem2.deregistrations(), 1);
        assert_eq!(elem2.first_known_vrf(), elem1.first_known_vrf());

        index.extend(&registers(1));
        let elem3 = index.get(&1).copied().unwrap();
        assert_eq!(elem3.registrations(), 2);
        assert_eq!(elem3.deregistrations(), 1);
        assert_eq!(elem3.first_known_vrf(), elem1.first_known_vrf());

        assert!(index.remove(&unregisters(1)));
        let elem4 = index.get(&1).copied().unwrap();
        assert_eq!(elem4.registrations(), 2);
        assert_eq!(elem4.deregistrations(), 0);

        assert!(index.remove(&registers(1)));
        assert!(index.remove(&registers(1)));
        assert!(index.get(&1).is_none(), "retracting the last registration drops the entry");
        assert!(!index.remove(&registers(1)), "removing non-existing element should return false");
    }
}
