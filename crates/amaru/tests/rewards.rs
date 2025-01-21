use std::path::PathBuf;

use amaru_ledger::{rewards::StakeDistributionSnapshot, store::RewardsSummary};
use amaru_stores::rocksdb::RocksDB;
use pallas_primitives::Epoch;

const LEDGER_DB: &str = "../../ledger.db";

fn compare_preprod_snapshot(epoch: Epoch) {
    let snapshot = StakeDistributionSnapshot::new(
        &RocksDB::from_snapshot(&PathBuf::from(LEDGER_DB), epoch).expect(format!("Failed to open ledger snapshot for epoch {}", epoch).as_str()),
    )
    .unwrap();
    insta::assert_json_snapshot!(format!("stake_distribution_{}", epoch), snapshot);
    let rewards_summary = RewardsSummary::new(
        &RocksDB::from_snapshot(&PathBuf::from(LEDGER_DB), epoch + 2).expect(format!("Failed to open ledger snapshot for epoch {}", epoch + 2).as_str()),
        snapshot,
    )
    .unwrap();
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
