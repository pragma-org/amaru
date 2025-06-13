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

use amaru_consensus::consensus::store::{ChainStore, StoreError};
use amaru_kernel::{protocol_parameters::GlobalParameters, Header, RationalNumber};
use amaru_ouroboros::{HasStakeDistribution, Nonces, PoolSummary};
use pallas_crypto::hash::Hash;
use serde::{Deserialize, Serialize};
use serde_json::Error;
use slot_arithmetic::{Epoch, Slot};
use std::{fs::File, io::BufReader, path::Path};

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

pub struct FakeStakeDistribution {
    total_active_stake: u64,
    pools: Vec<FakeStakePoolInfo>,
    max_kes_evolutions: u8,
    slots_per_kes_period: u64,
}

impl FakeStakeDistribution {
    pub fn from_file(
        stake_distribution_file: &Path,
        global_parameters: &GlobalParameters,
    ) -> Result<FakeStakeDistribution, Error> {
        let file = File::open(stake_distribution_file)
            .unwrap_or_else(|_| panic!("cannot find stake distribution file '{}', use --stake-distribution-file <FILE> to set the file to load distribution from", stake_distribution_file.display()));

        serde_json::from_reader(BufReader::new(file)).map(|pools: Vec<FakeStakePoolInfo>| {
            FakeStakeDistribution::new(pools, global_parameters)
        })
    }

    pub fn new(
        pools: Vec<FakeStakePoolInfo>,
        global_parameters: &GlobalParameters,
    ) -> FakeStakeDistribution {
        let total_active_stake = pools
            .iter()
            .map(|p| p.individual_stake.individual_total_pool_stake)
            .sum();

        FakeStakeDistribution {
            pools,
            total_active_stake,
            max_kes_evolutions: global_parameters.max_kes_evolution,
            slots_per_kes_period: global_parameters.slots_per_kes_period,
        }
    }
}

impl HasStakeDistribution for FakeStakeDistribution {
    fn get_pool(
        &self,
        _slot: amaru_kernel::Slot,
        pool: &amaru_kernel::PoolId,
    ) -> Option<amaru_ouroboros::PoolSummary> {
        self.pools
            .iter()
            .find(|p| p.pool_id == *pool)
            .map(|info| PoolSummary {
                vrf: info.individual_stake.individual_pool_stake_vrf,
                stake: info.individual_stake.individual_total_pool_stake,
                active_stake: self.total_active_stake,
            })
    }

    fn slot_to_kes_period(&self, slot: Slot) -> u64 {
        u64::from(slot) / self.slots_per_kes_period
    }

    fn max_kes_evolutions(&self) -> u64 {
        self.max_kes_evolutions as u64
    }

    fn latest_opcert_sequence_number(&self, pool: &amaru_kernel::PoolId) -> Option<u64> {
        self.pools
            .iter()
            .find(|p| p.pool_id == *pool)
            .map(|info| info.ocert_counter)
    }
}

#[derive(Debug)]
#[allow(dead_code)]
pub enum PopulateError {
    IoError(StoreError),
    SerdeError(serde_json::Error),
}

/// Populate a chain store with nonces data from given context file.
pub(crate) fn populate_chain_store(
    chain_store: &mut impl ChainStore<Header>,
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
#[allow(non_snake_case)]
pub struct ConsensusContext {
    active_slot_coeff: f64,
    nonce: Hash<32>,
    praos_max_KES_evo: u8,
    praos_slots_per_KES_period: u64,
}

#[cfg(test)]
mod test {
    use amaru_consensus::consensus::store::ChainStore;
    use amaru_kernel::network::NetworkName;
    use amaru_ouroboros::fake::FakeHeader;
    use amaru_stores::rocksdb::consensus::InMemConsensusStore;

    use super::populate_chain_store;

    use super::FakeStakeDistribution;
    use amaru_ouroboros::HasStakeDistribution;
    use pallas_crypto::hash::Hash;
    use rand::{rngs::StdRng, RngCore, SeedableRng};
    use std::path::PathBuf;

    /// FIXME: already exists in chain_selection test module
    pub fn random_bytes(arg: u32) -> Vec<u8> {
        let mut rng = StdRng::from_os_rng();
        let mut buffer = vec![0; arg as usize];
        rng.fill_bytes(&mut buffer);
        buffer
    }

    #[test]
    fn can_create_stake_distribution_from_file() {
        let stake_distribution_file = "tests/data/stake-distribution.json";
        let stake_distribution = FakeStakeDistribution::from_file(
            &PathBuf::from(stake_distribution_file),
            NetworkName::Preprod.into(),
        )
        .unwrap();
        let pool_id: Hash<28> = amaru_kernel::Hash::from(
            hex::decode("50484a702b93327308d85f51f1831940fdddcb751bf43bc3376c42b9")
                .unwrap()
                .as_slice(),
        );

        assert!(stake_distribution
            .get_pool(From::from(42), &pool_id)
            .is_some())
    }

    #[test]
    fn compute_total_stake_from_individual_pool_stake() {
        let stake_distribution_file = "tests/data/stake-distribution.json";
        let stake_distribution = FakeStakeDistribution::from_file(
            &PathBuf::from(stake_distribution_file),
            NetworkName::Preprod.into(),
        )
        .unwrap();
        let pool_id: Hash<28> = amaru_kernel::Hash::from(
            hex::decode("50484a702b93327308d85f51f1831940fdddcb751bf43bc3376c42b9")
                .unwrap()
                .as_slice(),
        );

        assert_eq!(
            Some(10000000000000000),
            stake_distribution
                .get_pool(From::from(42), &pool_id)
                .map(|p| p.active_stake)
        )
    }

    #[test]
    fn populate_chain_store_nonces_from_context_file() {
        let consensus_store_file = "tests/data/consensus-context.json";
        let mut consensus_store = InMemConsensusStore::new();
        let expected_nonce = amaru_kernel::Hash::from(
            hex::decode("ec08f270a044fb94bf61f9870e928a96cf75027d1f0e9f5dead0651b40849a89")
                .unwrap()
                .as_slice(),
        );
        let genesis_hash = random_bytes(32).as_slice().into();

        populate_chain_store(
            &mut consensus_store,
            &genesis_hash,
            &PathBuf::from(consensus_store_file),
        )
        .unwrap();

        assert_eq!(
            expected_nonce,
            <InMemConsensusStore as ChainStore<FakeHeader>>::get_nonces(
                &consensus_store,
                &genesis_hash
            )
            .unwrap()
            .active
        );
    }
}
