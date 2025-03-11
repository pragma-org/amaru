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
    governance::DRepsSummary,
    rewards::StakeDistribution,
    store::{RewardsSummary, Snapshot},
};
use amaru_stores::rocksdb::RocksDB;
use pallas_primitives::Epoch;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, LazyLock, Mutex},
};
use test_case::test_case;

pub static LEDGER_DB: LazyLock<PathBuf> = LazyLock::new(|| PathBuf::from("../../ledger.db"));

pub static CONNECTIONS: LazyLock<Mutex<BTreeMap<Epoch, Arc<RocksDB>>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

#[allow(clippy::panic)]
#[allow(clippy::unwrap_used)]
/// Get a read-only handle on a snapshot. This allows to run all test cases in parallel without
/// conflicts (a single scenario typically need 2 snapshots, so two tests may need access to the
/// same snapshot at the same time).
///
/// The following API ensures that this is handled properly, by creating connections only once and
/// sharing them safely between threads.
fn db(epoch: Epoch) -> Arc<impl Snapshot + Send + Sync> {
    let mut connections = CONNECTIONS.lock().unwrap();

    let handle = connections
        .entry(epoch)
        .or_insert_with(|| {
            Arc::new(
                RocksDB::for_epoch_with(&LEDGER_DB, epoch).unwrap_or_else(|_| {
                    panic!("Failed to open ledger snapshot for epoch {}", epoch)
                }),
            )
        })
        .clone();

    drop(connections);

    handle
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
// FIXME: re-enable once governance is implemented, we must be able to track proposal refunds in
// order to get those snapshots right.
//
// #[test_case(172)]
// #[test_case(173)]
// #[test_case(174)]
// #[test_case(175)]
// #[test_case(176)]
// #[test_case(177)]
// #[test_case(178)]
// #[test_case(179)]
#[ignore]
#[allow(clippy::unwrap_used)]
fn compare_preprod_snapshot(epoch: Epoch) {
    let stake_distr = StakeDistribution::new(db(epoch).as_ref()).unwrap();
    insta::assert_json_snapshot!(format!("stake_distribution_{}", epoch), stake_distr);

    let dreps = DRepsSummary::new(db(epoch).as_ref()).unwrap();
    insta::assert_json_snapshot!(format!("dreps_{}", epoch), dreps);

    // FIXME: remove this condition once we're able to proceed beyond Epoch 173.
    if epoch < 171 {
        let rewards_summary = RewardsSummary::new(db(epoch + 2).as_ref(), stake_distr).unwrap();
        insta::assert_json_snapshot!(format!("rewards_summary_{}", epoch), rewards_summary);
    }
}
