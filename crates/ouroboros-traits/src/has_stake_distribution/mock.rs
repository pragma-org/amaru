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

use crate::{HasStakeDistribution, PoolSummary};
use amaru_kernel::{Lovelace, PoolId, Slot, VrfKeyhash};
use std::collections::BTreeMap;

/// A mock implementing the HasStakeDistribution trait, suitable to validate a single block header
/// with default parameters.
pub struct MockLedgerState {
    pub vrf_vkey_hash: VrfKeyhash,
    pub stake: Lovelace,
    pub active_stake: Lovelace,
    pub op_certs: BTreeMap<PoolId, u64>,
    pub slots_per_kes_period: u64,
    pub max_kes_evolutions: u64,
}

impl MockLedgerState {
    #[allow(clippy::unwrap_used)]
    pub fn new(vrf_vkey_hash: &str, stake: Lovelace, active_stake: Lovelace) -> Self {
        Self {
            vrf_vkey_hash: vrf_vkey_hash.parse().unwrap(),
            stake,
            active_stake,
            op_certs: Default::default(),
            slots_per_kes_period: 129600, // (1.5 days in seconds)
            max_kes_evolutions: 62,
        }
    }
}

impl HasStakeDistribution for MockLedgerState {
    fn get_pool(&self, _slot: Slot, _pool: &PoolId) -> Option<PoolSummary> {
        Some(PoolSummary {
            vrf: self.vrf_vkey_hash,
            stake: self.stake,
            active_stake: self.active_stake,
        })
    }

    fn slot_to_kes_period(&self, slot: Slot) -> u64 {
        u64::from(slot) / self.slots_per_kes_period
    }

    fn max_kes_evolutions(&self) -> u64 {
        self.max_kes_evolutions
    }

    fn latest_opcert_sequence_number(&self, pool: &PoolId) -> Option<u64> {
        self.op_certs.get(pool).copied()
    }
}
