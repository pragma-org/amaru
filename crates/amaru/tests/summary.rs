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
    path::PathBuf,
    sync::{Arc, LazyLock, Mutex},
};

use amaru::default_snapshots_dir;
use amaru_kernel::{Epoch, NetworkName};
use amaru_ledger::{
    store::{ReadStore, Snapshot},
    summary::{governance::GovernanceSummary, rewards::RewardsSummary, stake_distribution::StakeDistribution},
};
use amaru_stores::rocksdb::{RocksDBHistoricalStores, RocksDBSnapshot, RocksDbConfig};
use anyhow::anyhow;
use test_case::test_case;

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

    let global_parameters = network.as_global_parameters().ok_or("no global parameters for network={network:?}?!")?;

    let era_history = network.as_era_history().ok_or("no era history for network={network:?}?!")?;

    let dreps = GovernanceSummary::new(snapshot.as_ref(), era_history)?;

    let stake_distr = StakeDistribution::new(snapshot.as_ref(), dreps)?;

    insta::with_settings!({
        snapshot_path => format!("stake-distributions/{}", network),
        omit_expression => true // do not include the default expression
    }, {
        insta::assert_json_snapshot!(
            format!("epoch_{epoch}"),
            stake_distr.for_network(network.into()),
        );
    });

    Ok(())
}

#[cfg(not(rust_analyzer))]
include!(concat!("stake-distributions/", env!("AMARU_NETWORK"), "/generated_test_cases.incl"));

// TODO: reinstate rewards summary snapshot tests
//
// let snapshot_from_the_future = db(network, epoch + 2);
// let protocol_parameters = snapshot_from_the_future.as_ref().protocol_parameters().unwrap();
// let rewards_summary =
//     RewardsSummary::new(snapshot_from_the_future.as_ref(), stake_distr, global_parameters, &protocol_parameters)
//         .unwrap()
//         .with_unclaimed_refunds(snapshot_from_the_future.as_ref())
//         .unwrap();
// insta::with_settings!({
//     snapshot_path => default_snapshots_dir(network)
// }, {
//     insta::assert_json_snapshot!(
//     format!("rewards_summary_{}", epoch),
//     rewards_summary
//     );
// });
