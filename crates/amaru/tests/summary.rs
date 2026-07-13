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

use std::{
    collections::BTreeMap,
    fs::File,
    io::Read,
    path::PathBuf,
    sync::{Arc, LazyLock, Mutex},
};

use amaru_kernel::{Epoch, NetworkName};
use amaru_ledger::{
    store::Snapshot,
    summary::{governance::GovernanceSummary, stake_distribution::StakeDistribution},
};
use amaru_stores::rocksdb::{RocksDBHistoricalStores, RocksDBSnapshot, RocksDbConfig};
use anyhow::anyhow;
use xz::read::XzDecoder;

const DEFAULT_AMARU_MAX_DIFFS: usize = 10;

pub static CONNECTIONS: LazyLock<Mutex<BTreeMap<Epoch, Arc<RocksDBSnapshot>>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

fn default_ledger_dir(network: NetworkName) -> String {
    format!("./ledger.{}.db", network.to_string().to_lowercase())
}

#[expect(clippy::panic)]
#[expect(clippy::unwrap_used)]
/// Get a read-only handle on a snapshot. This allows to run all test cases in parallel without
/// conflicts (a single scenario typically need 2 snapshots, so two tests may need access to the
/// same snapshot at the same time).
///
/// The following API ensures that this is handled properly, by creating connections only once and
/// sharing them safely between threads.
fn load_snapshot(network: NetworkName, epoch: Epoch) -> Arc<impl Snapshot + Send + Sync> {
    let mut connections = CONNECTIONS.lock().unwrap();

    let handle = connections
        .entry(epoch)
        .or_insert_with(|| {
            Arc::new(
                RocksDBHistoricalStores::for_epoch_with(
                    &RocksDbConfig::new(PathBuf::from(format!("../../{}", default_ledger_dir(network)))),
                    epoch,
                )
                .unwrap_or_else(|err| panic!("Failed to open ledger snapshot for epoch {}: {}", epoch, err)),
            )
        })
        .clone();

    drop(connections);

    handle
}

fn compare_stake_distribution_with_haskell_node(
    network: NetworkName,
    epoch: Epoch,
) -> Result<(), Box<dyn std::error::Error>> {
    let snapshot = load_snapshot(network, epoch);

    let era_history = network.as_era_history().ok_or("no era history for network={network:?}?!")?;

    let dreps = GovernanceSummary::new(snapshot.as_ref(), era_history)?;

    let stake_distr = StakeDistribution::new(snapshot.as_ref(), dreps)?;

    assert_json_snapshot(network, epoch, &stake_distr)
}

fn read_expected_snapshot(network: NetworkName, epoch: Epoch) -> Result<String, Box<dyn std::error::Error>> {
    let base_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("stake-distributions")
        .join(network.to_string())
        .join(format!("epoch_{epoch}.json"));

    if base_path.is_file() {
        return Ok(std::fs::read_to_string(base_path)?);
    }

    let compressed_path = base_path.with_extension("json.xz");
    if !compressed_path.is_file() {
        return Err(format!(
            "missing stake distribution snapshot: expected {} or {}",
            base_path.display(),
            compressed_path.display()
        )
        .into());
    }

    let mut decompressed = String::new();
    XzDecoder::new(File::open(compressed_path)?).read_to_string(&mut decompressed)?;
    Ok(decompressed)
}

#[allow(clippy::panic)]
fn assert_json_snapshot<T: serde::Serialize>(
    network: NetworkName,
    epoch: Epoch,
    actual: &T,
) -> Result<(), Box<dyn std::error::Error>> {
    let diffs = diff_json::compare_json(
        read_expected_snapshot(network, epoch)?.as_str(),
        serde_json::to_string_pretty(actual)?.as_str(),
    )?;

    let n: usize = std::env::var("AMARU_MAX_DIFFS")
        .map(|var| {
            var.parse::<usize>()
                .map_err(|e| anyhow!(e).context("invalid value for 'AMARU_MAX_DIFFS', must be a non-negative integer"))
        })
        .unwrap_or(Ok(DEFAULT_AMARU_MAX_DIFFS))?
        .min(diffs.len());

    if !diffs.is_empty() {
        let formatter = diff_json::DiffFormatter::new();
        panic!(
            "{}{}",
            formatter.format_compact(&diffs[0..n]),
            if diffs.len() > n { format!("...plus {} more difference(s)", diffs.len() - n) } else { String::new() }
        );
    }

    Ok(())
}

include!(concat!(env!("OUT_DIR"), "/stake_distribution_mainnet_test_cases.rs"));
include!(concat!(env!("OUT_DIR"), "/stake_distribution_preprod_test_cases.rs"));
include!(concat!(env!("OUT_DIR"), "/stake_distribution_preview_test_cases.rs"));
