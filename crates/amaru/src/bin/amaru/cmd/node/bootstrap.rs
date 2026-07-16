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

use std::{error::Error, fs::remove_dir_all, path::PathBuf};

use amaru::{bootstrap::bootstrap, default_chain_dir, default_ledger_dir};
use amaru_kernel::{Epoch, GlobalParameters, NetworkName, utils::path::relative_path};
use clap::{ArgAction, Parser};

#[derive(Debug, Parser)]
pub struct Args {
    /// Path of the chain on-disk storage.
    ///
    /// Defaults to ./chain.<NETWORK>.db when unspecified.
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::CHAIN_DIR,
    )]
    chain_dir: Option<PathBuf>,

    /// Forcefully erase and overwrite the ledger database if it already exists.
    #[arg(
        long,
        action = ArgAction::SetTrue,
        default_value_t = false,
    )]
    force: bool,

    /// Path of the ledger on-disk storage.
    ///
    /// Defaults to ./ledger.<NETWORK>.db when unspecified.
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::LEDGER_DIR,
    )]
    ledger_dir: Option<PathBuf>,

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

    /// Network to bootstrap the node for.
    #[arg(
        long,
        value_name = amaru::value_names::NETWORK,
        env = amaru::env_vars::NETWORK,
    )]
    network: NetworkName,

    /// Override network's global parameters for custom testnets.
    #[command(flatten)]
    global_parameters: GlobalParameters,

    /// Show global network parameter overrides, for custom testnets.
    #[arg(long)]
    pub(crate) help_global_parameters: bool,
}

macro_rules! info {
    ($name:literal $(, $($rest:tt)+)?) => {
        amaru_observability::info!(target: "amaru::cli", name: $name $(, $($rest)+)?);
    };
}

macro_rules! warn {
    ($name:literal $(, $($rest:tt)+)?) => {
        amaru_observability::warn!(target: "amaru::cli", name: $name $(, $($rest)+)?);
    };
}

pub async fn run(args: Args) -> Result<(), Box<dyn Error>> {
    let network = args.network;

    let global_parameters = network.as_global_parameters().cloned().unwrap_or(args.global_parameters);

    let ledger_dir = args.ledger_dir.unwrap_or_else(|| default_ledger_dir(network).into());

    let chain_dir = args.chain_dir.unwrap_or_else(|| default_chain_dir(network).into());

    info!(
        "node.bootstrap",
        force = %args.force,
        chain_dir = %relative_path(&chain_dir)?.display(),
        ledger_dir = %relative_path(&ledger_dir)?.display(),
        network = %network,
        epoch = args.epoch
            .map(|e| Box::new(e.to_string()) as Box<dyn tracing::Value>)
            .unwrap_or_else(|| Box::new(tracing::field::Empty)),
    );

    if ledger_dir.exists() || chain_dir.exists() {
        if !args.force {
            warn!(
                "snapshot.exist",
                hint = "ledger or chain directory already exists: use another location, remove it or use --force"
            );
            return Ok(());
        } else {
            if ledger_dir.exists() {
                warn!("ledger_db.forcefully_remove", dir=%relative_path(&ledger_dir)?.display());
                remove_dir_all(&ledger_dir)?;
            }
            if chain_dir.exists() {
                warn!("chain_db.forcefully_remove", dir=%relative_path(&chain_dir)?.display());
                remove_dir_all(&chain_dir)?;
            }
        }
    }

    bootstrap(network, &global_parameters, ledger_dir, chain_dir, args.epoch).await
}
