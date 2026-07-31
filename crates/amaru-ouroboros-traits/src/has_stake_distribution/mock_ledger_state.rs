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

use std::collections::BTreeMap;

use amaru_kernel::{Epoch, Hash, Lovelace, PoolId, size::VRF_KEY};

use crate::{PoolSummaries, PoolSummary};

/// Helper for tests. Use `to_pool_summaries` (with the pool id of the header issuer) to
/// obtain a `PoolSummaries` that will answer for the given pool.
pub struct MockLedgerState {
    pub vrf_vkey_hash: Hash<VRF_KEY>,
    pub stake: Lovelace,
    pub active_stake: Lovelace,
}

impl MockLedgerState {
    #[expect(clippy::unwrap_used)]
    pub fn new(vrf_vkey_hash: &str, stake: Lovelace, active_stake: Lovelace) -> Self {
        Self { vrf_vkey_hash: vrf_vkey_hash.parse().unwrap(), stake, active_stake }
    }

    /// Build a PoolSummaries that will return the mocked data for the specified pool at epoch 0.
    /// Tests that validate headers should ensure the era_history used with get_pool maps the
    /// header's slot such that (slot_epoch - 2) == 0, or populate additional epochs.
    pub fn to_pool_summaries(&self, pool: PoolId, epoch: Epoch) -> PoolSummaries {
        let mut pools = BTreeMap::new();
        pools.insert(pool, PoolSummary { vrf: self.vrf_vkey_hash, stake: self.stake, active_stake: self.active_stake });
        let mut by_epoch = BTreeMap::new();
        by_epoch.insert(epoch, pools);
        PoolSummaries { by_epoch }
    }
}
