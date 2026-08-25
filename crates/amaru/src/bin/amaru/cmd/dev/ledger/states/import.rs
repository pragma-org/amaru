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
    default_ledger_dir,
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
async fn run(args: Args) -> anyhow::Result<()> {
    let ledger_dir = args.ledger_dir.unwrap_or_else(|| default_ledger_dir(args.network).into());

    info!(
        cli::dev::RUN,
        command = "dev ledger states import",
        network = args.network,
        count = args.snapshot_paths.len(),
        ledger_dir = ledger_dir.to_string_lossy()
    );

    let global_parameters = args
        .network
        .as_global_parameters()
        .ok_or_else(|| anyhow!("no global parameters available for network {}", args.network))?;

    import_snapshots(args.network, global_parameters, &args.snapshot_paths, &ledger_dir).await?;

    println!("Imported {} snapshot(s) successfully", args.snapshot_paths.len());

    Ok(())
}
