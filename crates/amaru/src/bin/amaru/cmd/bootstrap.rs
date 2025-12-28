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

use amaru::{
    DEFAULT_NETWORK, bootstrap::bootstrap, default_chain_dir, default_ledger_dir,
    default_snapshots_dir,
};
use amaru_kernel::network::NetworkName;
use clap::Parser;
use std::{error::Error, fs::remove_dir_all, path::PathBuf};
use tracing::{error, info};

#[derive(Debug, Parser)]
pub struct Args {
    /// Path of the ledger on-disk storage.
    #[arg(long, value_name = "DIR", env = "AMARU_LEDGER_DIR")]
    ledger_dir: Option<PathBuf>,

    /// Path of the chain on-disk storage.
    #[arg(long, value_name = "DIR", env = "AMARU_CHAIN_DIR")]
    chain_dir: Option<PathBuf>,

    /// Network to bootstrap the node for.
    ///
    /// Should be one of 'mainnet', 'preprod', 'preview' or 'testnet_<magic>' where
    /// `magic` is a 32-bits unsigned value denoting a particular testnet.
    #[arg(
        long,
        value_name = "NETWORK",
        env = "AMARU_NETWORK",
        default_value_t = DEFAULT_NETWORK,
    )]
    network: NetworkName,

    #[arg(
        long,
        value_name = "DIR",
        verbatim_doc_comment,
        env = "AMARU_SNAPSHOTS_DIR"
    )]
    snapshots_dir: Option<PathBuf>,

    #[arg(
        short,
        long,
        value_name = "BOOL",
        verbatim_doc_comment,
        env = "AMARU_FORCE",
        default_value_t = false
    )]
    force: bool,
}

pub async fn run(args: Args) -> Result<(), Box<dyn Error>> {
    let network = args.network;

    let ledger_dir = args
        .ledger_dir
        .unwrap_or_else(|| default_ledger_dir(network).into());

    let chain_dir = args
        .chain_dir
        .unwrap_or_else(|| default_chain_dir(network).into());

    info!(network=%network, ledger_dir=%ledger_dir.to_string_lossy(), chain_dir=%chain_dir.to_string_lossy(),
          "Running command bootstrap",
    );

    let snapshots_dir = args
        .snapshots_dir
        .unwrap_or_else(|| default_snapshots_dir(network).into());

    let ledger_exists = ledger_dir.exists();
    if ledger_exists && !args.force {
        error!(ledger_dir=%ledger_dir.to_string_lossy(),  "Ledger directory already exists");
    } else {
        if args.force && ledger_exists {
            info!(ledger_dir=%ledger_dir.to_string_lossy(), "Forcing bootstrap, removing existing ledger directory");
            remove_dir_all(&ledger_dir)?;
        }
        bootstrap(network, ledger_dir, chain_dir, snapshots_dir).await?
    }

    Ok(())
}
