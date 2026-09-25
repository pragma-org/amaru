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

use std::{fs, io, path::Path};

use amaru::lifecycle::{Runnable, RuntimeKind};
use amaru_kernel::NetworkName;
use amaru_observability::info;
use anyhow::Context;
use clap::Parser;

#[derive(Debug, Parser)]
pub struct Args {
    /// Confirm removal of all node databases.
    #[arg(long, required = true)]
    wipe_all_dbs: bool,

    #[command(flatten)]
    network: amaru::args::Network,

    #[command(flatten, next_help_heading = "Storage options")]
    db_chain: amaru::args::DbChain,

    #[command(flatten, next_help_heading = "Storage options")]
    db_ledger: amaru::args::DbLedger,
}

pub(crate) fn runnable(args: Args) -> Runnable {
    Runnable::exit_on_signal(RuntimeKind::Simple, move || run(args))
}

async fn run(args: Args) -> anyhow::Result<()> {
    if !args.wipe_all_dbs {
        anyhow::bail!("refusing to remove node databases without --wipe-all-dbs");
    }

    let network = NetworkName::from(args.network);
    let db_ledger = args.db_ledger.into_path_buf(network);
    let db_chain = args.db_chain.into_path_buf(network);

    info!(
        cli::node::RM,
        db_chain = db_chain.display().to_string(),
        db_ledger = db_ledger.display().to_string(),
        network,
    );

    remove_database(&db_ledger)?;
    remove_database(&db_chain)?;

    Ok(())
}

fn remove_database(path: &Path) -> anyhow::Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        err => err.with_context(|| format!("failed to remove {}", path.display())),
    }
}
