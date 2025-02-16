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

use amaru_ledger::{
    rewards::StakeDistribution,
    store::{RewardsSummary, Store},
};
use amaru_stores::rocksdb::RocksDB;
use pallas_primitives::Epoch;
use std::{path::PathBuf, sync::LazyLock};
use test_case::test_case;

pub static LEDGER_DB: LazyLock<PathBuf> = LazyLock::new(|| PathBuf::from("../../ledger.db"));

#[allow(clippy::panic)]
fn open_db(epoch: Epoch) -> RocksDB {
    RocksDB::new(&LEDGER_DB)
        .unwrap_or_else(|_| panic!("Failed to open ledger snapshot for epoch {}", epoch))
}

#[test_case(163)]
#[test_case(164)]
#[test_case(165)]
#[test_case(166)]
#[test_case(167)]
#[test_case(168)]
#[test_case(169)]
#[test_case(170)]
#[test_case(171)]
#[test_case(172)]
#[test_case(173)]
#[test_case(174)]
#[test_case(175)]
#[test_case(176)]
#[test_case(177)]
#[test_case(178)]
#[test_case(179)]
#[ignore]
#[allow(clippy::unwrap_used)]
fn compare_preprod_snapshot(epoch: Epoch) {
    let db = open_db(epoch);

    let snapshot = StakeDistribution::new(&db.for_epoch(epoch).unwrap()).unwrap();
    insta::assert_json_snapshot!(format!("stake_distribution_{}", epoch), snapshot);

    let rewards_summary = RewardsSummary::new(&db.for_epoch(epoch + 2).unwrap(), snapshot).unwrap();
    insta::assert_json_snapshot!(format!("rewards_summary_{}", epoch), rewards_summary);
}
