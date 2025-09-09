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

use amaru_kernel::block::StageError;
use amaru_kernel::{Lovelace, Point, PoolId, RawBlock, VrfKeyhash};
use amaru_slot_arithmetic::Slot;

pub mod mock;

#[derive(Debug)]
pub struct PoolSummary {
    /// The blake2b-256 hash digest of the pool's VRF public key.
    pub vrf: VrfKeyhash,
    /// Total stake, in Lovelace, delegated to registered pools.
    pub active_stake: Lovelace,
    /// Stake of the underlying pool. The ratio stake/active_stake gives the pool's relative stake.
    pub stake: Lovelace,
}

/// The HasStakeDistribution trait provides a lookup mechanism for various information sourced from the ledger
pub trait HasStakeDistribution: Send + Sync {
    /// Obtain information about a pool such as its VRF key hash and its stake. The information is
    /// fetched from the ledger based on the given slot.
    fn get_pool(&self, slot: Slot, pool: &PoolId) -> Option<PoolSummary>;

    /// Calculate the KES period given an absolute slot and some shelley-genesis values
    fn slot_to_kes_period(&self, slot: Slot) -> u64;

    /// Get the maximum number of KES evolutions from the ledger state
    fn max_kes_evolutions(&self) -> u64;

    /// Get the latest opcert sequence number we've seen for a given issuer_vkey
    ///
    /// TODO: This should most probably live within the consensus, and not the ledger, similar to
    /// the tracking of the epoch nonce.
    fn latest_opcert_sequence_number(&self, pool: &PoolId) -> Option<u64>;
}

pub trait HasBlockValidation: Send + Sync {
    fn roll_forward_block(
        &self,
        point: &Point,
        block: &RawBlock,
    ) -> Result<Result<u64, StageError>, StageError>;

    fn rollback_block(&self, to: &Point) -> Result<(), StageError>;
}

/// A fake block fetcher that always returns an empty block.
/// This is used in for simulating the network.
#[derive(Clone, Debug, Default)]
pub struct FakeBlockValidation;

impl HasBlockValidation for FakeBlockValidation {
    fn roll_forward_block(
        &self,
        _point: &Point,
        _block: &RawBlock,
    ) -> Result<Result<u64, StageError>, StageError> {
        Ok(Ok(1))
    }

    fn rollback_block(&self, _to: &Point) -> Result<(), StageError> {
        Ok(())
    }
}
