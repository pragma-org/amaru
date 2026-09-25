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

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use amaru::{
    default_chain_dir, default_ledger_dir,
    lifecycle::{Runnable, RuntimeKind},
};
use amaru_kernel::NetworkName;
use amaru_observability::info;
use anyhow::Context;
use clap::Parser;

#[derive(Debug, Parser)]
pub struct Args {
    /// Confirm removal of all node databases.
    #[arg(long, required = true)]
    wipe_all_dbs: bool,

    /// Path of the chain on-disk storage.
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::CHAIN_DB,
        alias = "chain-dir",
    )]
    chain_db: Option<PathBuf>,

    /// Path of the ledger on-disk storage.
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::LEDGER_DB,
        alias = "ledger-dir",
    )]
    ledger_db: Option<PathBuf>,

    /// Network whose node databases should be removed.
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

async fn run(args: Args) -> anyhow::Result<()> {
    let Args { wipe_all_dbs, chain_db, ledger_db, network } = args;
    if !wipe_all_dbs {
        anyhow::bail!("refusing to remove node databases without --wipe-all-dbs");
    }

    let ledger_db = ledger_db.unwrap_or_else(|| default_ledger_dir(network).into());
    let chain_db = chain_db.unwrap_or_else(|| default_chain_dir(network).into());

    info!(
        cli::node::RM,
        chain_db = chain_db.display().to_string(),
        ledger_db = ledger_db.display().to_string(),
        network,
    );

    remove_database(&ledger_db)?;
    remove_database(&chain_db)?;

    Ok(())
}

fn remove_database(path: &Path) -> anyhow::Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        err => err.with_context(|| format!("failed to remove {}", path.display())),
    }
}
