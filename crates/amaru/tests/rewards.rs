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

pub static LEDGER_DB: LazyLock<PathBuf> = LazyLock::new(|| PathBuf::from("../../ledger.db"));

fn open_db(epoch: Epoch) -> RocksDB {
    RocksDB::new(&LEDGER_DB)
        .unwrap_or_else(|_| panic!("Failed to open ledger snapshot for epoch {}", epoch))
}

fn compare_preprod_snapshot(epoch: Epoch) {
    let db = open_db(epoch);

    let snapshot = StakeDistribution::new(&db.for_epoch(epoch).unwrap()).unwrap();
    insta::assert_json_snapshot!(format!("stake_distribution_{}", epoch), snapshot);

    let rewards_summary = RewardsSummary::new(&db.for_epoch(epoch + 2).unwrap(), snapshot).unwrap();
    insta::assert_json_snapshot!(format!("rewards_summary_{}", epoch), rewards_summary);
}

#[test]
#[ignore]
fn compare_preprod_snapshot_163() {
    compare_preprod_snapshot(163)
}

#[test]
#[ignore]
fn compare_preprod_snapshot_164() {
    compare_preprod_snapshot(164)
}

#[test]
#[ignore]
fn compare_preprod_snapshot_165() {
    compare_preprod_snapshot(165)
}

#[test]
#[ignore]
fn compare_preprod_snapshot_166() {
    compare_preprod_snapshot(166)
}

#[test]
#[ignore]
fn compare_preprod_snapshot_167() {
    compare_preprod_snapshot(167)
}

#[test]
#[ignore]
fn compare_preprod_snapshot_168() {
    compare_preprod_snapshot(168)
}

#[test]
#[ignore]
fn compare_preprod_snapshot_169() {
    compare_preprod_snapshot(169)
}

#[test]
#[ignore]
fn compare_preprod_snapshot_170() {
    compare_preprod_snapshot(170)
}

#[test]
#[ignore]
fn compare_preprod_snapshot_171() {
    compare_preprod_snapshot(171)
}

#[test]
#[ignore]
fn compare_preprod_snapshot_172() {
    compare_preprod_snapshot(172)
}

#[test]
#[ignore]
fn compare_preprod_snapshot_173() {
    compare_preprod_snapshot(173)
}

#[test]
#[ignore]
fn compare_preprod_snapshot_174() {
    compare_preprod_snapshot(174)
}

#[test]
#[ignore]
fn compare_preprod_snapshot_175() {
    compare_preprod_snapshot(175)
}

#[test]
#[ignore]
fn compare_preprod_snapshot_176() {
    compare_preprod_snapshot(176)
}

#[test]
#[ignore]
fn compare_preprod_snapshot_177() {
    compare_preprod_snapshot(177)
}

#[test]
#[ignore]
fn compare_preprod_snapshot_178() {
    compare_preprod_snapshot(178)
}

#[test]
#[ignore]
fn compare_preprod_snapshot_179() {
    compare_preprod_snapshot(179)
}
