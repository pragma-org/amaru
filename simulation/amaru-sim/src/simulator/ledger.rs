// Copyright 2025 PRAGMA
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

use amaru_kernel::RationalNumber;
use amaru_ouroboros::{
    BlockHeader, ChainStore, HasStakeDistribution, Nonces, PoolSummary, StoreError,
};
use amaru_slot_arithmetic::{Epoch, Slot};
use pallas_crypto::hash::Hash;
use serde::{Deserialize, Serialize};
use serde_json::Error;
use std::sync::Arc;
use std::{fs::File, io::BufReader, path::Path};

/// A fake stake distribution used for simulation purposes.
/// It can be loaded from a JSON file or instantiated directly from a list of pools (FakeStakePoolInfo).
pub struct FakeStakeDistribution {
    total_active_stake: u64,
    pools: Vec<FakeStakePoolInfo>,
}

impl FakeStakeDistribution {
    /// Deserialize a stake distribution from a JSON file.
    pub fn from_file(stake_distribution_file: &Path) -> Result<FakeStakeDistribution, Error> {
        let file = File::open(stake_distribution_file)
            .unwrap_or_else(|_| panic!("cannot find stake distribution file '{}', use --stake-distribution-file <FILE> to set the file to load distribution from", stake_distribution_file.display()));

        serde_json::from_reader(BufReader::new(file))
            .map(|pools: Vec<FakeStakePoolInfo>| FakeStakeDistribution::new(pools))
    }

    /// Create a new stake distribution from a list of pools.
    pub fn new(pools: Vec<FakeStakePoolInfo>) -> FakeStakeDistribution {
        let total_active_stake = pools
            .iter()
            .map(|p| p.individual_stake.individual_total_pool_stake)
            .sum();

        FakeStakeDistribution {
            pools,
            total_active_stake,
        }
    }
}

impl HasStakeDistribution for FakeStakeDistribution {
    fn get_pool(&self, _slot: Slot, pool: &amaru_kernel::PoolId) -> Option<PoolSummary> {
        self.pools
            .iter()
            .find(|p| p.pool_id == *pool)
            .map(|info| PoolSummary {
                vrf: info.individual_stake.individual_pool_stake_vrf,
                stake: info.individual_stake.individual_total_pool_stake,
                active_stake: self.total_active_stake,
            })
    }
}

/// Minimum pool information for testing the system.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FakeStakePoolInfo {
    cold_sign_key: String,
    individual_stake: IndividualStake,
    kes_sign_key: String,
    ocert_counter: u64,
    pool_id: Hash<28>,
    pool_idx: u64,
    vrf_sign_key: String,
}

/// Stake data for a single pool.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndividualStake {
    /// The ratio of stake for a single pool.
    individual_pool_stake: RationalNumber,

    /// The hash of the VRF key for the pool.
    individual_pool_stake_vrf: Hash<32>,

    /// The total stake for the pool.
    individual_total_pool_stake: u64,
}

#[derive(Debug)]
#[expect(dead_code)]
pub enum PopulateError {
    IoError(StoreError),
    SerdeError(serde_json::Error),
}

/// Populate a chain store with nonces data from given context file.
pub(crate) fn populate_chain_store(
    chain_store: Arc<dyn ChainStore<BlockHeader>>,
    header: &Hash<32>,
    consensus_context_file: &Path,
) -> Result<(), PopulateError> {
    use PopulateError::*;

    let file = File::open(consensus_context_file)
        .unwrap_or_else(|_| panic!("cannot find consensus context file '{}', use --consensus-context-file <FILE> to set the file to load context from", consensus_context_file.display()));

    let store: ConsensusContext =
        serde_json::from_reader(BufReader::new(file)).map_err(SerdeError)?;
    let nonces = Nonces {
        active: store.nonce,
        evolving: store.nonce,
        candidate: store.nonce,
        tail: *header,
        epoch: Epoch::from(0),
    };

    chain_store.put_nonces(header, &nonces).map_err(IoError)?;

    Ok(())
}

/// Consensus context used to populate chain store with data needed
/// for consensus.
///
/// This context is generated as part of traces generation from Haskell
/// code.
/// At this stage (2025-03-18) we do not use the `active_slot_coeff` field.
#[derive(Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
// NOTE: we allow non snake case here because of the way the field name and KES
// acronym is usually encoded in Haskell-land.
#[expect(non_snake_case)]
pub struct ConsensusContext {
    active_slot_coeff: f64,
    nonce: Hash<32>,
    praos_max_KES_evo: u8,
    praos_slots_per_KES_period: u64,
}

#[cfg(test)]
mod test {
    use super::*;

    use amaru_kernel::Slot;
    use amaru_kernel::tests::random_bytes;
    use amaru_ouroboros::HasStakeDistribution;
    use amaru_ouroboros::in_memory_consensus_store::InMemConsensusStore;
    use pallas_crypto::hash::Hash;
    use std::path::PathBuf;

    #[test]
    fn can_create_stake_distribution_from_file() {
        let stake_distribution_file = "tests/data/stake-distribution.json";
        let stake_distribution =
            FakeStakeDistribution::from_file(&PathBuf::from(stake_distribution_file)).unwrap();
        let pool_id: Hash<28> = amaru_kernel::Hash::from(
            hex::decode("50484a702b93327308d85f51f1831940fdddcb751bf43bc3376c42b9")
                .unwrap()
                .as_slice(),
        );

        assert!(
            stake_distribution
                .get_pool(Slot::from(42), &pool_id)
                .is_some()
        )
    }

    #[test]
    fn compute_total_stake_from_individual_pool_stake() {
        let stake_distribution_file = "tests/data/stake-distribution.json";
        let stake_distribution =
            FakeStakeDistribution::from_file(&PathBuf::from(stake_distribution_file)).unwrap();
        let pool_id: Hash<28> = amaru_kernel::Hash::from(
            hex::decode("50484a702b93327308d85f51f1831940fdddcb751bf43bc3376c42b9")
                .unwrap()
                .as_slice(),
        );

        assert_eq!(
            Some(10000000000000000),
            stake_distribution
                .get_pool(Slot::from(42), &pool_id)
                .map(|p| p.active_stake)
        )
    }

    #[test]
    fn populate_chain_store_nonces_from_context_file() {
        let consensus_store_file = "tests/data/consensus-context.json";
        let consensus_store: Arc<dyn ChainStore<BlockHeader>> =
            Arc::new(InMemConsensusStore::new());
        let expected_nonce = amaru_kernel::Hash::from(
            hex::decode("ec08f270a044fb94bf61f9870e928a96cf75027d1f0e9f5dead0651b40849a89")
                .unwrap()
                .as_slice(),
        );
        let genesis_hash = random_bytes(32).as_slice().into();

        populate_chain_store(
            consensus_store.clone(),
            &genesis_hash,
            &PathBuf::from(consensus_store_file),
        )
        .unwrap();

        assert_eq!(
            expected_nonce,
            consensus_store.get_nonces(&genesis_hash).unwrap().active
        );
    }
}
