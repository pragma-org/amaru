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

use std::fs;

use amaru::lifecycle::{Runnable, RuntimeKind};
use amaru_kernel::{Epoch, NetworkName};
use amaru_ledger::state::MIN_LEDGER_SNAPSHOTS;
use amaru_observability::{info, warn};
use amaru_stores::rocksdb::RocksDB;
use anyhow::Context;
use clap::Parser;

#[derive(Debug, Parser)]
pub struct Args {
    /// The epochs to remove.
    #[arg(value_name = amaru::value_names::UINT)]
    epochs: Vec<Epoch>,

    #[command(flatten)]
    db_ledger: amaru::args::DbLedger,

    #[command(flatten)]
    network: amaru::args::Network,
}

pub(crate) fn runnable(args: Args) -> Runnable {
    Runnable::exit_on_signal(RuntimeKind::Simple, move || run(args))
}

#[expect(clippy::print_stdout)]
async fn run(args: Args) -> anyhow::Result<()> {
    let network = NetworkName::from(args.network);
    let db_ledger = args.db_ledger.into_path_buf(network);

    info!(cli::dev::ledger::state::REMOVE, db_ledger = db_ledger.to_string_lossy(), network);

    let existing = RocksDB::snapshots(&db_ledger)?;
    let remaining_after = existing.iter().filter(|e| !args.epochs.contains(e)).count();

    if remaining_after < MIN_LEDGER_SNAPSHOTS as usize {
        anyhow::bail!(
            "refusing to remove: would leave only {} snapshots (minimum required: {})",
            remaining_after,
            MIN_LEDGER_SNAPSHOTS
        );
    }

    let mut removed = 0u64;
    for epoch in &args.epochs {
        let epoch_dir = db_ledger.join(format!("{epoch}"));
        if epoch_dir.exists() {
            fs::remove_dir_all(&epoch_dir).with_context(|| format!("failed to remove {}", epoch_dir.display()))?;
            info!(cli::dev::ledger::SNAPSHOT_REMOVED, epoch = u64::from(*epoch));
            removed += 1;
        } else {
            warn!(cli::dev::ledger::SNAPSHOT_NOT_FOUND, epoch = u64::from(*epoch));
        }
    }

    println!("Removed {removed} snapshot(s)");

    Ok(())
}
