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
    alloc::System,
    collections::BTreeMap,
    env,
    fmt::Write,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
};

use amaru::env_vars;
use amaru_kernel::{Epoch, NetworkName, PREPROD_ERA_HISTORY, utils::memory};
use amaru_ledger::{
    store::Snapshot,
    summary::{governance::GovernanceSummary, stake_distribution::StakeSummary},
};
use amaru_stores::rocksdb::{RocksDB, RocksDBHistoricalStores, RocksDBSnapshot, RocksDbConfig};
use anyhow::anyhow;
use zstd::stream::read::Decoder as ZstdDecoder;

const DEFAULT_AMARU_MAX_DIFFS: usize = 10;
const REPORT_COLUMN_WIDTH: usize = 14;
const REPORT_LABEL_WIDTH: usize = 20;

#[global_allocator]
static GLOBAL_ALLOCATOR: memory::CountingAllocator<System> = memory::CountingAllocator::new(System);

pub static CONNECTIONS: LazyLock<Mutex<BTreeMap<Epoch, Arc<RocksDBSnapshot>>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

/// Path to the workspace-root ledger DB for `network`, from the summary test binary cwd.
fn ledger_dir_from_tests(network: NetworkName) -> PathBuf {
    PathBuf::from(format!("../../ledger.{}.db", network.to_string().to_lowercase()))
}

/// Whether `ledger_dir` contains a RocksDB snapshot directory for `epoch`.
fn has_ledger_snapshot(ledger_dir: &Path, epoch: Epoch) -> bool {
    ledger_dir.join(epoch.to_string()).is_dir()
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
                RocksDBHistoricalStores::for_epoch_with(&RocksDbConfig::new(ledger_dir_from_tests(network)), epoch)
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
    let ledger_dir = ledger_dir_from_tests(network);
    if !has_ledger_snapshot(&ledger_dir, epoch) {
        // Soft-skip: missing snapshots used to be `#[ignore]`d at build time. Print a warning so
        // `--nocapture` (or harness failure logs) still surfaces the gap.
        eprintln!(
            "warning: skipping stake distribution comparison for {network} epoch {epoch}; \
             local ledger snapshot missing at {}",
            ledger_dir.join(epoch.to_string()).display()
        );
        return Ok(());
    }

    let snapshot = load_snapshot(network, epoch);

    let era_history = network.as_era_history().ok_or("no era history for network={network:?}?!")?;

    let dreps = GovernanceSummary::new(snapshot.as_ref(), era_history)?;

    let stake_summary = StakeSummary::new(snapshot.as_ref(), dreps, network, |_| {})?;

    assert_json_snapshot(network, epoch, &stake_summary)
}

#[test]
// NOTE: To see the output of this test, pass `--no-capture` to the test runner.
fn measure_new_snapshot_summary_memory() -> Result<(), Box<dyn std::error::Error>> {
    let network = env::var(env_vars::NETWORK)
        .ok()
        .map(|network| network.parse::<NetworkName>())
        .transpose()?
        .unwrap_or(NetworkName::Preprod);

    let era_history = network.as_era_history().unwrap_or(&PREPROD_ERA_HISTORY);

    let ledger_dir = ledger_dir_from_tests(network);
    if !ledger_dir.is_dir() {
        eprintln!("skipping summary memory measurement; ledger directory {} does not exist", ledger_dir.display());
        return Ok(());
    }

    let Some(epoch) = RocksDB::snapshots(&ledger_dir)?.into_iter().next_back() else {
        eprintln!(
            "skipping summary memory measurement; ledger directory {} does not contain any epoch snapshots",
            ledger_dir.display()
        );
        return Ok(());
    };

    let snapshot = RocksDBHistoricalStores::for_epoch_with(&RocksDbConfig::new(ledger_dir), epoch)?;

    let allocated_before = GLOBAL_ALLOCATOR.snapshot();

    let (mut summary, summary_rss_mib) = memory::rss_delta(|| {
        let governance = GovernanceSummary::new(&snapshot, era_history).unwrap_or_else(|e| panic!("{e}"));
        StakeSummary::new(&snapshot, governance, network, |_| {}).unwrap_or_else(|e| panic!("{e}"))
    });
    let summary_allocated_bytes =
        GLOBAL_ALLOCATOR.current_allocated_bytes().saturating_sub(allocated_before.current_allocated_bytes);
    let summary_peak_allocated_bytes =
        GLOBAL_ALLOCATOR.peak_allocated_bytes().saturating_sub(allocated_before.peak_allocated_bytes);

    let accounts_len = summary.accounts.len();
    let pools_len = summary.pools.len();
    let dreps_len = summary.dreps.len();

    let (mut stake_distribution, accounts_rss_mib) = std::hint::black_box(memory::rss_delta(move || {
        let stake_distribution = std::mem::take(&mut summary.stake_distribution);
        drop(summary);
        stake_distribution
    }));
    let stake_distribution_allocated_bytes =
        GLOBAL_ALLOCATOR.current_allocated_bytes().saturating_sub(allocated_before.current_allocated_bytes);
    let stake_distribution_rss_mib = summary_rss_mib - accounts_rss_mib.abs();
    let accounts_allocated_bytes = summary_allocated_bytes.saturating_sub(stake_distribution_allocated_bytes);

    let (mut stake_distribution, pools_rss_mib) = std::hint::black_box(memory::rss_delta(move || {
        drop(std::mem::take(&mut stake_distribution.pools));
        stake_distribution
    }));
    let without_pools_allocated_bytes =
        GLOBAL_ALLOCATOR.current_allocated_bytes().saturating_sub(allocated_before.current_allocated_bytes);
    let pools_allocated_bytes = stake_distribution_allocated_bytes.saturating_sub(without_pools_allocated_bytes);

    let (stake_distribution, dreps_rss_mib) = std::hint::black_box(memory::rss_delta(move || {
        drop(std::mem::take(&mut stake_distribution.dreps));
        stake_distribution
    }));
    let without_dreps_allocated_bytes =
        GLOBAL_ALLOCATOR.current_allocated_bytes().saturating_sub(allocated_before.current_allocated_bytes);
    let dreps_allocated_bytes = without_pools_allocated_bytes.saturating_sub(without_dreps_allocated_bytes);

    drop(stake_distribution);

    let mut report = String::new();

    let _ = writeln!(report, "=============== STAKE SUMMARY MEMORY USAGE REPORT ===============");
    let _ = writeln!(report, "network{:<width$} {}", "", network, width = REPORT_LABEL_WIDTH - "network".len());
    let _ = writeln!(report, "epoch{:<width$} {}", "", epoch, width = REPORT_LABEL_WIDTH - "epoch".len());
    let _ = writeln!(report);
    let _ = writeln!(
        report,
        "{:<label_width$} {:<width$} {:<width$} {:<width$}",
        "summary",
        format!("rss={summary_rss_mib}MiB"),
        format!("alloc={}", format_bytes(summary_allocated_bytes)),
        format!("peak={}", format_bytes(summary_peak_allocated_bytes)),
        label_width = REPORT_LABEL_WIDTH,
        width = REPORT_COLUMN_WIDTH,
    );
    let _ = writeln!(
        report,
        "{:<label_width$} {:<width$} {:<width$}",
        "stake_distribution",
        format!("rss={stake_distribution_rss_mib}MiB"),
        format!("alloc={}", format_bytes(stake_distribution_allocated_bytes)),
        label_width = REPORT_LABEL_WIDTH,
        width = REPORT_COLUMN_WIDTH,
    );
    let _ = writeln!(report);
    let _ = writeln!(
        report,
        "{:<label_width$} {:<width$} {:<width$} {:<width$}",
        "accounts",
        format!("rss={}MiB", accounts_rss_mib.abs()),
        format!("alloc={}", format_bytes(accounts_allocated_bytes)),
        format!("count={accounts_len}"),
        label_width = REPORT_LABEL_WIDTH,
        width = REPORT_COLUMN_WIDTH,
    );
    let _ = writeln!(
        report,
        "{:<label_width$} {:<width$} {:<width$} {:<width$}",
        "pools",
        format!("rss={}MiB", pools_rss_mib.abs()),
        format!("alloc={}", format_bytes(pools_allocated_bytes)),
        format!("count={pools_len}"),
        label_width = REPORT_LABEL_WIDTH,
        width = REPORT_COLUMN_WIDTH,
    );
    let _ = writeln!(
        report,
        "{:<label_width$} {:<width$} {:<width$} {:<width$}",
        "dreps",
        format!("rss={}MiB", dreps_rss_mib.abs()),
        format!("alloc={}", format_bytes(dreps_allocated_bytes)),
        format!("count={dreps_len}"),
        label_width = REPORT_LABEL_WIDTH,
        width = REPORT_COLUMN_WIDTH,
    );
    eprintln!("{report}");

    Ok(())
}

fn format_bytes(bytes: usize) -> String {
    const KIB: usize = 1024;
    const MIB: usize = 1024 * 1024;

    if bytes >= MIB {
        format!("{}MiB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{}KiB", bytes / KIB)
    } else {
        format!("{bytes}B")
    }
}

fn read_expected_snapshot(network: NetworkName, epoch: Epoch) -> Result<String, Box<dyn std::error::Error>> {
    let base_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("conformance")
        .join("stake-distributions")
        .join(network.to_string())
        .join(format!("epoch_{epoch}.json"));

    if base_path.is_file() {
        return Ok(std::fs::read_to_string(base_path)?);
    }

    let compressed_path = base_path.with_extension("json.zst");
    if !compressed_path.is_file() {
        return Err(format!(
            "missing stake distribution snapshot: expected {} or {}",
            base_path.display(),
            compressed_path.display()
        )
        .into());
    }

    let mut decompressed = String::new();
    ZstdDecoder::new(File::open(compressed_path)?)?.read_to_string(&mut decompressed)?;
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
            if diffs.len() > n {
                format!("...plus {} more difference(s); set 'AMARU_MAX_DIFFS' to see more.", diffs.len() - n)
            } else {
                String::new()
            }
        );
    }

    Ok(())
}

include!(concat!(env!("OUT_DIR"), "/stake_distribution_mainnet_test_cases.rs"));
include!(concat!(env!("OUT_DIR"), "/stake_distribution_preprod_test_cases.rs"));
include!(concat!(env!("OUT_DIR"), "/stake_distribution_preview_test_cases.rs"));
