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

use amaru_kernel::{Epoch, Lovelace, PoolId, VrfKeyhash};
use amaru_slot_arithmetic::{EraHistoryError, Slot};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub mod mock_ledger_state;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolSummary {
    /// The blake2b-256 hash digest of the pool's VRF public key.
    pub vrf: VrfKeyhash,
    /// Total stake, in Lovelace, delegated to registered pools.
    pub active_stake: Lovelace,
    /// Stake of the underlying pool. The ratio stake/active_stake gives the pool's relative stake.
    pub stake: Lovelace,
}

#[derive(Debug, Error, serde::Serialize, serde::Deserialize)]
pub enum GetPoolError {
    #[error("slot to epoch conversion failed {0}.")]
    SlotToEpochConversionFailure(#[from] EraHistoryError),
    #[error("no stake distribution available for pool access {0}.")]
    StakeDistributionNotAvailable(Epoch),
}

/// The HasStakeDistribution trait provides a lookup mechanism for various information sourced from the ledger
pub trait HasStakeDistribution: Send + Sync {
    /// Obtain information about a pool such as its VRF key hash and its stake. The information is
    /// fetched from the ledger based on the given slot.
    fn get_pool(&self, slot: Slot, pool: &PoolId) -> Result<Option<PoolSummary>, GetPoolError>;
}
