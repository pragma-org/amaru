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

use std::{fs::File, io::BufReader, path::Path};

use amaru_kernel::RationalNumber;
use amaru_ledger::{BlockValidationResult, ValidateBlockEvent};
use amaru_ouroboros::{HasStakeDistribution, PoolSummary};
use gasket::framework::*;
use pallas_crypto::hash::Hash;
use serde::{Deserialize, Serialize};
use serde_json::Error;

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
}

impl FakeStakeDistribution {
    pub fn from_file(stake_distribution_file: &Path) -> Result<FakeStakeDistribution, Error> {
        let file = File::open(stake_distribution_file).unwrap_or_else(|_| panic!("cannot find stake distribution file '{}', use --stake-distribution-file <FILE> to set the file to load distribution from", stake_distribution_file.display()));

        serde_json::from_reader(BufReader::new(file))
            .map(|pools: Vec<FakeStakePoolInfo>| FakeStakeDistribution::new(pools))
    }

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

    fn slot_to_kes_period(&self, _slot: u64) -> u64 {
        todo!()
    }

    fn max_kes_evolutions(&self) -> u64 {
        todo!()
    }

    fn latest_opcert_sequence_number(&self, _pool: &amaru_kernel::PoolId) -> Option<u64> {
        todo!()
    }
}

// Fake ledger

type UpstreamPort = gasket::messaging::InputPort<ValidateBlockEvent>;

type DownstreamPort = gasket::messaging::OutputPort<BlockValidationResult>;

#[derive(Stage)]
#[stage(name = "pull", unit = "ValidateBlockEvent", worker = "Worker")]
pub struct FakeLedgerStage {
    pub upstream: UpstreamPort,
    pub downstream: DownstreamPort,
}

impl FakeLedgerStage {
    pub fn new() -> Self {
        Self {
            upstream: UpstreamPort::default(),
            downstream: DownstreamPort::default(),
        }
    }
}

pub struct Worker {}

#[async_trait::async_trait(?Send)]
impl gasket::framework::Worker<FakeLedgerStage> for Worker {
    async fn bootstrap(_stage: &FakeLedgerStage) -> Result<Self, WorkerError> {
        let worker = Self {};

        Ok(worker)
    }

    async fn schedule(
        &mut self,
        stage: &mut FakeLedgerStage,
    ) -> Result<WorkSchedule<ValidateBlockEvent>, WorkerError> {
        let event = stage.upstream.recv().await.or_panic()?;
        Ok(WorkSchedule::Unit(event.payload))
    }

    async fn execute(
        &mut self,
        unit: &ValidateBlockEvent,
        stage: &mut FakeLedgerStage,
    ) -> Result<(), WorkerError> {
        let event: BlockValidationResult = match unit {
            ValidateBlockEvent::Validated(point, _vec, span) => {
                BlockValidationResult::BlockValidated(point.clone(), span.clone())
            }
            ValidateBlockEvent::Rollback(point) => {
                BlockValidationResult::RolledBackTo(point.clone())
            }
        };
        stage.downstream.send(event.into()).await.or_panic()?;
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::FakeStakeDistribution;
    use amaru_ouroboros::HasStakeDistribution;
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

        assert!(stake_distribution.get_pool(42, &pool_id).is_some())
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
            Some(1250000000000000),
            stake_distribution
                .get_pool(42, &pool_id)
                .map(|p| p.active_stake)
        )
    }
}
