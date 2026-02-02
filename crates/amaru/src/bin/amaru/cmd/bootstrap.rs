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

use amaru::{DEFAULT_NETWORK, bootstrap::bootstrap, default_chain_dir, default_ledger_dir};
use amaru_kernel::NetworkName;
use clap::{ArgAction, Parser};
use std::{error::Error, fs::remove_dir_all, path::PathBuf};
use tracing::{info, warn};

#[derive(Debug, Parser)]
pub struct Args {
    /// Path of the chain on-disk storage.
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::CHAIN_DIR,
    )]
    chain_dir: Option<PathBuf>,

    /// Forcefully erase and overwrite the ledger database if it already exists.
    #[arg(
        short,
        long,
        action = ArgAction::SetTrue,
        default_value_t = false,
    )]
    force: bool,

    /// Path of the ledger on-disk storage.
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::LEDGER_DIR,
    )]
    ledger_dir: Option<PathBuf>,

    /// Network to bootstrap the node for.
    #[arg(
        long,
        value_name = amaru::value_names::NETWORK,
        env = amaru::env_vars::NETWORK,
        default_value_t = DEFAULT_NETWORK,
    )]
    network: NetworkName,
}

pub async fn run(args: Args) -> Result<(), Box<dyn Error>> {
    let network = args.network;

    let ledger_dir = args
        .ledger_dir
        .unwrap_or_else(|| default_ledger_dir(network).into());

    let chain_dir = args
        .chain_dir
        .unwrap_or_else(|| default_chain_dir(network).into());

    info!(
        _command = "bootstrap",
        chain_dir = %chain_dir.to_string_lossy(),
        force = %args.force,
        ledger_dir = %ledger_dir.to_string_lossy(),
        network = %network,
        "running",
    );

    if ledger_dir.exists() {
        if !args.force {
            warn!(
                ledger_dir=%ledger_dir.to_string_lossy(),
                "ledger directory already exists"
            );
            return Ok(());
        } else {
            info!(
                ledger_dir=%ledger_dir.to_string_lossy(),
                "forcing bootstrap, removing existing ledger directory"
            );
            remove_dir_all(&ledger_dir)?;
        }
    }

    bootstrap(network, ledger_dir, chain_dir).await
}
