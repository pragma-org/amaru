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

use std::{error::Error, path::PathBuf};

use amaru_kernel::network::NetworkName;
use clap::{arg, Parser};

use super::import_ledger_state::import_all_from_directory;

#[derive(Debug, Parser)]
pub struct Args {
    /// Path of the on-disk storage.
    /// This directory will be created if it does not exist, and will
    /// contain the databases needed for the node to run,
    #[arg(long, value_name = "DIR", default_value = ".")]
    base_dir: PathBuf,

    /// Network to bootstrap the node for.
    ///
    /// Should be one of 'mainnet', 'preprod', 'preview' or 'testnet:<magic>' where
    /// `magic` is a 32-bits unsigned value denoting a particular testnet.
    #[arg(
        long,
        value_name = "NETWORK",
        default_value_t = NetworkName::Preprod,
    )]
    network: NetworkName,
}

pub async fn run(args: Args) -> Result<(), Box<dyn Error>> {
    let network = args.network;
    let era_history = network.into();

    let ledger_dir = args.base_dir.join("ledger.db");
    let _chain_dir = args.base_dir.join("chain.db");

    let snapshots_file: PathBuf = ["data", &*network.to_string(), "snapshots.json"].iter().collect();
    let snapshots_dir: PathBuf = PathBuf::from(network.to_string());

    download_snapshots(&snapshots_file, &snapshots_dir).await?;

    import_all_from_directory(&ledger_dir, era_history, &snapshots_dir).await
}

async fn download_snapshots(_snapshots_file: &PathBuf, _snapshots_dir: &PathBuf) -> Result<(), Box<dyn Error>> {
    todo!()
}
