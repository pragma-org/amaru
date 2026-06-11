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

use amaru_kernel::{Epoch, EraHistoryError, Hash, Lovelace, PoolId, Slot, size::VRF_KEY};
use pallas_math::math::FixedDecimal;
use thiserror::Error;

/// The HasPools trait provides a lookup mechanism for various information sourced from the pools
/// registered in the ledger.
pub trait HasPools: Send + Sync {
    /// Obtain information about a pool such as its VRF key hash and its stake. The information is
    /// fetched from the ledger based on the given slot.
    fn get_pool_summary(&self, slot: Slot, pool: &PoolId) -> Result<Option<PoolSummary>, GetPoolError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PoolSummary {
    /// The blake2b-256 hash digest of the pool's VRF public key.
    vrf_key: Hash<VRF_KEY>,
    /// Stake of the underlying pool. The ratio stake/active_stake gives the pool's relative stake.
    stake: Lovelace,
    /// Total stake, in Lovelace, delegated to registered pools.
    active_stake: Lovelace,
}

impl PoolSummary {
    pub fn new(vrf_key: Hash<VRF_KEY>, stake: Lovelace, active_stake: Lovelace) -> Self {
        Self { vrf_key, stake, active_stake }
    }

    pub fn vrf_key(&self) -> Hash<VRF_KEY> {
        self.vrf_key
    }

    pub fn relative_stake(&self) -> FixedDecimal {
        assert!(self.active_stake != 0, "the active_stake cannot be 0");
        FixedDecimal::from(self.stake) / FixedDecimal::from(self.active_stake)
    }
}

#[derive(Debug, Error, serde::Serialize, serde::Deserialize)]
pub enum GetPoolError {
    #[error("slot to epoch conversion failed {0}.")]
    SlotToEpochConversionFailure(#[from] EraHistoryError),
    #[error("no stake distribution available for pool for slot {0} at epoch {1}.")]
    StakeDistributionNotAvailable(Slot, Epoch),
    #[error("cannot get the pool details from the store.")]
    StoreError(#[from] crate::StoreError),
}
