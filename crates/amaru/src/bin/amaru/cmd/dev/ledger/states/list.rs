// Copyright 2026 PRAGMA
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

use std::{path::PathBuf, time::SystemTime};

use amaru::{
    default_ledger_dir,
    lifecycle::{Runnable, RuntimeKind},
};
use amaru_kernel::{NetworkName, Point};
use amaru_ledger::store::{HistoricalStores, ReadStore};
use amaru_observability::info;
use amaru_stores::rocksdb::{RocksDBHistoricalStores, RocksDbConfig};
use clap::Parser;

#[derive(Debug, Parser)]
pub struct Args {
    /// The path to the ledger database.
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::LEDGER_DIR,
    )]
    ledger_dir: Option<PathBuf>,

    /// Network of the underlying ledger database.
    #[arg(
        long,
        value_name = amaru::value_names::NETWORK,
        env = amaru::env_vars::NETWORK,
    )]
    network: NetworkName,
}

pub(crate) fn runnable(args: Args) -> Runnable {
    Runnable::exit_on_signal(RuntimeKind::Simple, move || run(args))
}

#[expect(clippy::print_stdout)]
async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let ledger_dir = args.ledger_dir.unwrap_or_else(|| default_ledger_dir(args.network).into());

    info!(
        cli::dev::RUN,
        command = "dev ledger states list",
        network = args.network,
        ledger_dir = ledger_dir.to_string_lossy()
    );

    let era_history = args.network.as_era_history();
    let global_params = args.network.as_global_parameters();
    let system_start =
        global_params.map(|gp| SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(gp.system_start));

    let config = RocksDbConfig::new(ledger_dir);
    let historical = RocksDBHistoricalStores::new(&config, 0);
    let snapshots = historical.snapshots()?;

    if snapshots.is_empty() {
        println!("No ledger state snapshots found.");
        return Ok(());
    }

    println!("{:<10} {:<60} TIMESTAMP", "EPOCH", "POINT");
    println!("{}", "-".repeat(90));

    for epoch in &snapshots {
        let snapshot = RocksDBHistoricalStores::for_epoch_with(&config, *epoch)?;
        let tip = snapshot.tip()?;

        let timestamp = match (&tip, era_history, system_start) {
            (Point::Specific(slot, _, _), Some(eh), Some(ss)) => eh
                .slot_to_relative_time_unchecked_horizon(*slot)
                .ok()
                .map(|rel| ss + rel)
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map(|d| format_timestamp(d.as_secs()))
                .unwrap_or_else(|| "-".to_string()),
            _ => "-".to_string(),
        };

        println!("{:<10} {:<60} {}", u64::from(*epoch), tip, timestamp);
    }

    println!("\n=> {} snapshots", snapshots.len());

    Ok(())
}

fn format_timestamp(unix_secs: u64) -> String {
    let secs_per_day = 86400u64;
    let secs_per_hour = 3600u64;
    let secs_per_minute = 60u64;

    let days_since_epoch = unix_secs / secs_per_day;
    let remaining = unix_secs % secs_per_day;
    let hours = remaining / secs_per_hour;
    let minutes = (remaining % secs_per_hour) / secs_per_minute;
    let seconds = remaining % secs_per_minute;

    let (year, month, day) = days_to_ymd(days_since_epoch);

    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}

fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    let mut y = 1970u64;
    let mut remaining = days;

    loop {
        let days_in_year = if is_leap_year(y) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        y += 1;
    }

    let months_days: [u64; 12] = if is_leap_year(y) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut m = 0usize;
    for days_in_month in &months_days {
        if remaining < *days_in_month {
            break;
        }
        remaining -= days_in_month;
        m += 1;
    }

    (y, (m + 1) as u64, remaining + 1)
}

fn is_leap_year(y: u64) -> bool {
    (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400)
}
