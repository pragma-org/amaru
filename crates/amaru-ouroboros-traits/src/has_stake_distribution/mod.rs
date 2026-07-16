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

use amaru_kernel::{Epoch, EraHistory, EraHistoryError, Hash, Lovelace, PoolId, Slot, num::CheckedSub, size::VRF_KEY};
use thiserror::Error;

pub mod mock_ledger_state;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PoolSummary {
    /// The blake2b-256 hash digest of the pool's VRF public key.
    pub vrf: Hash<VRF_KEY>,
    /// Total stake, in Lovelace, delegated to registered pools.
    pub active_stake: Lovelace,
    /// Stake of the underlying pool. The ratio stake/active_stake gives the pool's relative stake.
    pub stake: Lovelace,
}

#[derive(Debug, Error, serde::Serialize, serde::Deserialize)]
pub enum GetPoolError {
    #[error("slot to epoch conversion failed {0}.")]
    SlotToEpochConversionFailure(#[from] EraHistoryError),
    #[error(
        "no leader stake distribution available for pool at slot {0}{hint}",
        hint = match .1 {
            Some(epoch) => format!(" at epoch {epoch}"),
            None => "; too old".to_string(),
        }
    )]
    StakeDistributionNotAvailable(Slot, Option<Epoch>),
}

/// PoolSummaries holds (up to three) projected maps of `PoolId -> PoolSummary` derived from the
/// ledger's stake distributions (only the small `.pools` data, not the large accounts map).
///
/// This replaces the previous `HasStakeDistribution` trait for header validation. The ledger
/// projects the required values; the struct is cheap to clone and can be passed as a resource
/// or effect input.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct PoolSummaries {
    /// Keyed by the epoch of the corresponding stake distribution snapshot.
    pub by_epoch: BTreeMap<Epoch, BTreeMap<PoolId, PoolSummary>>,
}

impl PoolSummaries {
    pub fn max_epoch(&self) -> Epoch {
        self.by_epoch.last_key_value().map(|(e, _)| *e).unwrap_or(*Epoch::ZERO)
    }

    /// Obtain information about a pool such as its VRF key hash and its stake.
    /// The epoch is derived from the slot using the same rule as before (slot_epoch - 2).
    pub fn get_pool(
        &self,
        slot: Slot,
        pool: &PoolId,
        era_history: &EraHistory,
    ) -> Result<Option<PoolSummary>, GetPoolError> {
        let target_epoch = era_history
            .slot_to_epoch_unchecked_horizon(slot)
            .map_err(GetPoolError::SlotToEpochConversionFailure)?
            .checked_sub(Epoch::TWO);

        let pools = target_epoch
            .and_then(|e| self.by_epoch.get(&e))
            .ok_or(GetPoolError::StakeDistributionNotAvailable(slot, target_epoch))?;

        Ok(pools.get(pool).copied())
    }
}
