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

// Fake stake distribution

// {
//   "chain": "dd6bb6e544c08294a95f2adcf8fe2766067e15eb8e31c1cdb0287f68e9346e0d",
//   "coldSignKey": "9d90778c89b3287651d9a85770732fc031ebdebf3334d1e1da7519a75f2fd15f",
//   "individualStake": {
//     "individualPoolStake": {
//       "denominator": 10,
//       "numerator": 1
//     },
//     "individualPoolStakeVrf": "650c43fc40bd6430cc5902e6e0555d91a1fdf14122272b92ae8e27ccf9675009",
//     "individualTotalPoolStake": 1000000000000000
//   },
//   "kesSignKey": "86c3972eb54d56a5fc4d892e0353cfa5882575e8c9b59ca7e46c2560a58a15e20ee09b2c885ef7ae4dec8b00a60c766b8fdf68858ba2af863d802b6000f86dd2a83cfed24ad8e97da87d6c75fa334522615d1dcedeb0caf754b5364e29864d81be13107f43adea6bc5e361b6904fb84316655f062b819f6b254418f78225b2bc553b10e7e788c9756e00de4043070353f034198e5c530721786c29e3e3db3b3fe0d7eff0156b448dcaec38a967e689404984e7b2e2330317b17e88566141550db26a5b7705b3240c5fc17c19bbce87ea90fb7b0c60bb182373ec484e7dd0809c6c9c1e2a4c532302845f6f3ac580facfa9e17d4c56617da0239d7d12e310a1ccb96daf53de9beca8dd6fa7ba6fcb31461b0b59d8b9a10691979d1b99a323721f941f60a444f28426920c2751323ba28b365c9a7db4471231043fa43b3ded1e43755615a87ae050662b0f59dfbd0575aa3774d619982f1fe34f798796b1d721943e685dd28d54f7d32dc695be8c2840a4f1d41e14d5cc21891747c35dc45c57204ba6608a0e792e90b3bc8699ca829bd07b469d276dea25e2aab185bc648d2aca1658ac77b073e2929170904650021dd9be5e0fdb9f953dec4687c4c53d772437718af1c87edbe43ad31d9febae2e518a320cf4700dfbcf6e2ac1634db5f7a5b337c0523b1cc7be656963be1bd94c8da1683a8bc737c3f8e0d45219b90a2faa9ba6244ee9aab869697da9c8cd398dc9cf3711315afde182194a0a0377fd8aa8e34fe26eaffa9bbe1a14592c3ebcf693a6dd65edeeaebf1a664536f98dfa7b9a3491170cf1b06e846af02edf754206372b3d77bf250bebbeec9bc22ada298b4e1b",
//   "ocertCounter": 47,
//   "poolId": "650c43fc40bd6430cc5902e6e0555d91a1fdf14122272b92ae8e27ccf9675009",
//   "poolIdx": 1,
//   "vrfSignKey": "97bc9670f28ad98b9cd3a2c02fdb504a831756163dd19dc0fc983ebe7f1df51f3ef06e6b84c658e419219cd960f20885fdb889e37b0323940ea5a2b904db9c97"
// }

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndividualStake {
    individual_pool_stake: RationalNumber,
    individual_pool_stake_vrf: Hash<32>,
    individual_total_pool_stake: u64,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FakeStakePoolInfo {
    cold_sign_key: String,
    individual_stake: IndividualStake,
    kes_sign_key: String,
    ocert_counter: u64,
    pool_id: Hash<28>,
    pool_idx: u64,
    vrf_sign_key: String,
}

impl FakeStakePoolInfo {
    fn stake(&self) -> u64 {
        let RationalNumber {
            denominator,
            numerator,
        } = self.individual_stake.individual_pool_stake;
        numerator * self.individual_stake.individual_total_pool_stake / denominator
    }
}

pub struct FakeStakeDistribution {
    pools: Vec<FakeStakePoolInfo>,
}

impl FakeStakeDistribution {
    pub fn from_file(stake_distribution_file: &Path) -> Result<FakeStakeDistribution, Error> {
        let file = File::open(stake_distribution_file).unwrap();
        serde_json::from_reader(BufReader::new(file)).map(|pools| FakeStakeDistribution { pools })
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
                stake: info.stake(),
                active_stake: info.individual_stake.individual_total_pool_stake,
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
}
