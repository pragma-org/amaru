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

use crate::cmd::{default_chain_dir, default_ledger_dir};
use amaru::{bootstrap::bootstrap, default_snapshots_dir};
use amaru_kernel::network::NetworkName;
use clap::Parser;
use std::{error::Error, path::PathBuf};
use tracing::{debug, info};

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
        default_value_t = super::DEFAULT_NETWORK,
    )]
    network: NetworkName,

    /// Path to directory containing per-network bootstrap configuration files.
    ///
    /// This path will be used as a prefix to resolve per-network configuration files
    /// needed for bootstrapping. Given a source directory `data`, and a
    /// a network name of `preview`, the expected layout for configuration files would be:
    ///
    /// * `data/preview/snapshots.json`: a list of `Snapshot` values,
    /// * `data/preview/nonces.json`: a list of `InitialNonces` values,
    /// * `data/preview/headers.json`: a list of `Point`s.
    #[arg(
        long,
        value_name = "DIR",
        default_value = super::DEFAULT_CONFIG_DIR,
        verbatim_doc_comment,
        env = "AMARU_CONFIG_DIR"
    )]
    config_dir: PathBuf,

    #[arg(
        long,
        value_name = "DIR",
        verbatim_doc_comment,
        env = "AMARU_SNAPSHOT_DIR"
    )]
    snapshots_dir: Option<PathBuf>,
}

pub async fn run(args: Args) -> Result<(), Box<dyn Error>> {
    let network = args.network;

    let ledger_dir = args
        .ledger_dir
        .unwrap_or_else(|| default_ledger_dir(network).into());

    let chain_dir = args
        .chain_dir
        .unwrap_or_else(|| default_chain_dir(network).into());

    info!(config=%args.config_dir.to_string_lossy(), ledger_dir=%ledger_dir.to_string_lossy(), chain_dir=%chain_dir.to_string_lossy(), network=%network,
          "Running command bootstrap",
    );

    let network_dir = args.config_dir.join(&*network.to_string());
    let snapshots_dir = args
        .snapshots_dir
        .unwrap_or_else(|| default_snapshots_dir(network).into());

    if !bootstrap(network, ledger_dir, chain_dir, network_dir, snapshots_dir).await? {
        debug!("Already bootstrapped; skipping");
    }

    Ok(())
}
