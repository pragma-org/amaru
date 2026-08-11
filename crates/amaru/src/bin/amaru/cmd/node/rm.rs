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
    error::Error,
    fs, io,
    path::{Path, PathBuf},
};

use amaru::{
    default_chain_dir, default_ledger_dir,
    lifecycle::{Runnable, RuntimeKind},
};
use amaru_kernel::NetworkName;
use amaru_observability::info;
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
        env = amaru::env_vars::CHAIN_DIR,
    )]
    chain_dir: Option<PathBuf>,

    /// Path of the ledger on-disk storage.
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::LEDGER_DIR,
    )]
    ledger_dir: Option<PathBuf>,

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

async fn run(args: Args) -> Result<(), Box<dyn Error>> {
    let Args { wipe_all_dbs, chain_dir, ledger_dir, network } = args;
    if !wipe_all_dbs {
        return Err("refusing to remove node databases without --wipe-all-dbs".into());
    }

    let ledger_dir = ledger_dir.unwrap_or_else(|| default_ledger_dir(network).into());
    let chain_dir = chain_dir.unwrap_or_else(|| default_chain_dir(network).into());

    info!(
        cli::node::RM,
        chain_dir = %chain_dir.display(),
        ledger_dir = %ledger_dir.display(),
        network = network,
    );

    remove_database(&ledger_dir)?;
    remove_database(&chain_dir)?;

    Ok(())
}

fn remove_database(path: &Path) -> Result<(), io::Error> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(io::Error::new(err.kind(), format!("failed to remove {}: {err}", path.display()))),
    }
}
