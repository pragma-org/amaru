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

use amaru_kernel::{Hash, Lovelace, PoolId, Slot, size::VRF_KEY};

use crate::{GetPoolError, HasPools, PoolSummary};

/// A mock implementing the HasPools trait, suitable to validate a single block header
/// with default parameters.
pub struct MockLedgerState {
    pub vrf_vkey_hash: Hash<VRF_KEY>,
    pub stake: Lovelace,
    pub active_stake: Lovelace,
}

impl HasPools for MockLedgerState {
    fn get_pool_summary(&self, _slot: Slot, _pool_id: &PoolId) -> Result<Option<PoolSummary>, GetPoolError> {
        Ok(Some(PoolSummary::new(self.vrf_vkey_hash, self.stake, self.active_stake)))
    }
}
