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

use std::path::PathBuf;

use amaru::{DEFAULT_NETWORK, default_chain_dir};
use amaru_kernel::{NetworkName, Point};
use amaru_stores::rocksdb::{RocksDbConfig, consensus::RocksDBStore};

#[derive(Debug, clap::Parser)]
pub struct Args {
    /// The epoch to reset to
    #[arg(
        value_name = amaru::value_names::POINT,
    )]
    blocks: Vec<Point>,

    /// The path to the chain store database to remove the validation status from
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::CHAIN_DIR,
    )]
    chain_dir: Option<PathBuf>,

    /// Network of the underlying chain database.
    #[arg(
        long,
        value_name = amaru::value_names::NETWORK,
        env = amaru::env_vars::NETWORK,
        default_value_t = DEFAULT_NETWORK,
    )]
    network: NetworkName,
}

pub async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let chain_dir = args.chain_dir.unwrap_or_else(|| default_chain_dir(args.network).into());

    tracing::info!(
        _command = "remove-validation-status",
        chain_dir = %chain_dir.to_string_lossy(),
        network = %args.network,
        "running",
    );

    let chain_store = RocksDBStore::open(&RocksDbConfig::new(chain_dir))?;

    for block in args.blocks {
        tracing::info!(%block, "removing block validation status");
        chain_store.remove_block_valid(&block.hash())?;
    }

    Ok(())
}
