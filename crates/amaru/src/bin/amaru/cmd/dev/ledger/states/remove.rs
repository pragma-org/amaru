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

use std::{fs, path::PathBuf};

use amaru::{
    default_ledger_dir,
    lifecycle::{Runnable, RuntimeKind},
};
use amaru_kernel::{Epoch, NetworkName};
use amaru_ledger::state::MIN_LEDGER_SNAPSHOTS;
use amaru_observability::{info, warn};
use amaru_stores::rocksdb::RocksDB;
use clap::Parser;

#[derive(Debug, Parser)]
pub struct Args {
    /// The epochs to remove.
    #[arg(value_name = amaru::value_names::UINT)]
    epochs: Vec<Epoch>,

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
        command = "dev ledger states remove",
        network = args.network,
        ledger_dir = ledger_dir.to_string_lossy()
    );

    let existing = RocksDB::snapshots(&ledger_dir)?;
    let remaining_after = existing.iter().filter(|e| !args.epochs.contains(e)).count();

    if remaining_after < MIN_LEDGER_SNAPSHOTS as usize {
        return Err(format!(
            "refusing to remove: would leave only {} snapshots (minimum required: {})",
            remaining_after, MIN_LEDGER_SNAPSHOTS
        )
        .into());
    }

    let mut removed = 0u64;
    for epoch in &args.epochs {
        let epoch_dir = ledger_dir.join(format!("{epoch}"));
        if epoch_dir.exists() {
            fs::remove_dir_all(&epoch_dir).map_err(|e| format!("failed to remove {}: {e}", epoch_dir.display()))?;
            info!(cli::dev::ledger::SNAPSHOT_REMOVED, epoch = u64::from(*epoch));
            removed += 1;
        } else {
            warn!(cli::dev::ledger::SNAPSHOT_NOT_FOUND, epoch = u64::from(*epoch));
        }
    }

    println!("Removed {removed} snapshot(s)");

    Ok(())
}
