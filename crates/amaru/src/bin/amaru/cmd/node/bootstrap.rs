// Copyright 2025 PRAGMA
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

use std::{fs, io, path::Path};

use amaru::{
    aws::S3Config,
    bootstrap::bootstrap,
    default_snapshots_dir,
    lifecycle::{Runnable, RuntimeKind},
};
use amaru_kernel::{Epoch, GlobalParameters, NetworkName, utils::path::relative_path};
use amaru_observability::{info, warn};
use clap::Parser;

#[derive(Debug, Parser)]
pub struct Args {
    /// The target bootstrap epoch; this is the epoch Amaru will start from.
    ///
    /// At least 3 past epochs must exist. When omitted, this defaults the latest available epoch
    /// from known snapshots.
    #[arg(
        long = "epoch",
        value_name = amaru::value_names::UINT,
        env = amaru::env_vars::EPOCH,
    )]
    epoch: Option<Epoch>,

    #[command(flatten)]
    network: amaru::args::Network,

    #[command(flatten, next_help_heading = "Storage options")]
    db_chain: amaru::args::DbChain,

    #[command(flatten, next_help_heading = "Storage options")]
    db_ledger: amaru::args::DbLedger,

    #[command(flatten)]
    s3: amaru::args::S3,

    /// Override network's global parameters for custom testnets / devnets.
    ///
    /// DO NOT override for known networks (e.g. mainnet, preprod, preview, ...), as these parameters are set in stone.
    #[command(flatten, next_help_heading = "Network global parameters")]
    global_parameters: GlobalParameters,

    /// Show global network parameter overrides, for custom testnets.
    #[arg(long)]
    pub(crate) help_global_parameters: bool,
}

pub(crate) fn runnable(args: Args) -> Runnable {
    Runnable::exit_on_signal(RuntimeKind::Io, move || run(args))
}

async fn run(args: Args) -> anyhow::Result<()> {
    let network = NetworkName::from(args.network);

    let global_parameters = network.as_global_parameters().cloned().unwrap_or(args.global_parameters);

    let db_ledger = args.db_ledger.into_path_buf(network);

    let db_chain = args.db_chain.into_path_buf(network);

    info!(
        cli::node::BOOTSTRAP,
        db_chain = relative_path(&db_chain)?.display().to_string(),
        db_ledger = relative_path(&db_ledger)?.display().to_string(),
        network,
        epoch = @args.epoch.map(|e| e.to_string()),
    );

    let ledger_db_populated = is_populated(&db_ledger)?;
    let chain_db_populated = is_populated(&db_chain)?;

    if ledger_db_populated || chain_db_populated {
        let mut messages = Vec::new();

        if ledger_db_populated {
            let dir = relative_path(&db_ledger)?.display().to_string();
            let hint = "ledger directory already exists: use another location or remove it manually";
            warn!(cli::db_ledger::EXIST, dir, hint);
            messages.push(format!("{hint} ({dir})"));
        }

        if chain_db_populated {
            let dir = relative_path(&db_chain)?.display().to_string();
            let hint = "chain directory already exists: use another location or remove it manually";
            warn!(cli::db_chain::EXIST, dir, hint);
            messages.push(format!("{hint} ({dir})"));
        }

        anyhow::bail!("{}", messages.join("; "));
    }

    bootstrap(
        network,
        &global_parameters,
        db_ledger,
        db_chain,
        default_snapshots_dir(network).into(),
        args.epoch,
        S3Config {
            bucket: args.s3.bucket,
            endpoint: args.s3.endpoint,
            region: args.s3.region,
            public_url: args.s3.public_url,
        },
        amaru_bootstrap::BootstrapCancellation::new(),
    )
    .await?;

    Ok(())
}

/// Whether `dir` holds anything. An empty directory is no more a database than a missing one, and
/// the build script keeps empty ledger directories around for cargo to watch.
///
/// A directory that cannot be read is not reported as empty: this guards existing databases, so it
/// must not let a bootstrap through on a failed inspection.
fn is_populated(dir: &Path) -> Result<bool, io::Error> {
    match fs::read_dir(dir) {
        Ok(mut entries) => Ok(entries.next().is_some()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(io::Error::new(err.kind(), format!("{}: {err}", dir.display()))),
    }
}
