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

use std::path::PathBuf;

use amaru::{
    bootstrap::import_snapshots,
    lifecycle::{Runnable, RuntimeKind},
};
use amaru_kernel::NetworkName;
use amaru_observability::info;
use anyhow::anyhow;
use clap::Parser;

#[derive(Debug, Parser)]
pub struct Args {
    /// Path(s) to the snapshot(s) to import (CBOR file or cardano-node snapshot directory).
    #[arg(value_name = amaru::value_names::FILEPATH, required = true)]
    snapshot_paths: Vec<PathBuf>,

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

    info!(
        cli::dev::ledger::state::IMPORT,
        count = args.snapshot_paths.len(),
        db_ledger = db_ledger.to_string_lossy(),
        network,
    );

    let global_parameters = network
        .as_global_parameters()
        .ok_or_else(|| anyhow!("no global parameters available for network {network}"))?;

    import_snapshots(network, global_parameters, &args.snapshot_paths, &db_ledger).await?;

    println!("Imported {} snapshot(s) successfully", args.snapshot_paths.len());

    Ok(())
}
