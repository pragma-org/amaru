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

use std::path::PathBuf;

use amaru::{
    DEFAULT_NETWORK,
    bootstrap::{InitialNonces, default_bootstrap_nonces, store_nonces},
    default_chain_dir,
};
use amaru_kernel::{EraHistory, NetworkName};
use amaru_stores::rocksdb::{RocksDbConfig, consensus::RocksDBStore};
use clap::Parser;
use tracing::info;

#[derive(Debug, Parser)]
pub struct Args {
    /// Path of the consensus on-disk storage.
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::CHAIN_DIR
    )]
    chain_dir: Option<PathBuf>,

    /// Network the nonces are imported for.
    #[arg(
        long,
        value_name = amaru::value_names::NETWORK,
        env = amaru::env_vars::NETWORK,
        default_value_t = DEFAULT_NETWORK,
    )]
    network: NetworkName,

    /// JSON-formatted file with manually specified nonces details.
    ///
    /// When not present, initial nonces are derived from the configured bootstrap snapshots for
    /// the selected network.
    #[arg(
        long,
        value_name = amaru::value_names::FILEPATH,
        env = amaru::env_vars::NONCES_FILE,
    )]
    nonces_file: Option<PathBuf>,
}

pub async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let network = args.network;

    let chain_dir = args.chain_dir.unwrap_or_else(|| default_chain_dir(network).into());

    info!(
        _command = "import-nonces",
        chain_dir = %chain_dir.to_string_lossy(),
        network = %network,
        "running",
    );

    let (epoch, initial_nonces) = if let Some(manually_specified_file) = args.nonces_file {
        let initial_nonces: InitialNonces = serde_json::from_slice(std::fs::read(manually_specified_file)?.as_slice())?;

        let era_history: &EraHistory = network.into();
        let epoch = era_history.slot_to_epoch_unchecked_horizon(initial_nonces.at.slot_or_default())?;

        (epoch, initial_nonces)
    } else {
        default_bootstrap_nonces(network)?
    };

    let chain_db = RocksDBStore::open_and_migrate(&RocksDbConfig::new(chain_dir.clone()))?;

    store_nonces(epoch, &chain_db, initial_nonces)
}
