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

/// Convert a Haskell cardano-node state for import into Amaru.
///
/// This command accepts a cardano-node snapshot directory (containing `state` and `tables/tvar`
/// files) or a CBOR snapshot file, and imports it into the specified Amaru ledger database.
///
/// For cardano-node InMem snapshots, the expected directory layout is:
///
///   <snapshot-dir>/
///     state          (the serialized ledger state)
///     tables/
///       tvar         (the UTxO table)
#[derive(Debug, Parser)]
pub struct Args {
    /// Path to the cardano-node snapshot (directory with `state` + `tables/tvar`, or CBOR file).
    #[arg(value_name = amaru::value_names::FILEPATH)]
    input: PathBuf,

    /// The path to the output Amaru ledger database.
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::LEDGER_DB,
        alias = "ledger-dir",
    )]
    ledger_db: Option<PathBuf>,

    /// Network of the snapshot being converted.
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
    let ledger_db = args.ledger_db.unwrap_or_else(|| default_ledger_dir(args.network).into());

    info!(
        cli::dev::ledger::CONVERT,
        input = args.input.to_string_lossy(),
        ledger_db = ledger_db.to_string_lossy(),
        network = args.network,
    );

    let global_parameters = args
        .network
        .as_global_parameters()
        .ok_or_else(|| anyhow!("no global parameters available for network {}", args.network))?;

    import_snapshots(args.network, global_parameters, std::slice::from_ref(&args.input), &ledger_db).await?;

    println!("Converted and imported snapshot from {} into {}", args.input.display(), ledger_db.display());

    Ok(())
}
