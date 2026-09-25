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

use amaru::{
    default_chain_dir,
    lifecycle::{Runnable, RuntimeKind},
};
use amaru_kernel::NetworkName;
use amaru_observability::info;
use amaru_ouroboros::WriteChainStore;
use amaru_stores::rocksdb::{RocksDbConfig, consensus::RocksDBStore};

use crate::cmd::PointOrHash;

#[derive(Debug, clap::Parser)]
pub struct Args {
    /// The blocks from which to remove the validation status
    #[arg(
        value_name = amaru::value_names::POINT_OR_HASH,
    )]
    blocks: Vec<PointOrHash>,

    /// The path to the chain store database to remove the validation status from
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::CHAIN_DB,
        alias = "chain-dir",
    )]
    chain_db: Option<PathBuf>,

    /// Network of the underlying chain database.
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
    let chain_db = args.chain_db.unwrap_or_else(|| default_chain_dir(args.network).into());

    info!(
        cli::dev::chain::CLEAR_INVALID,
        blocks = args.blocks.iter().map(|block| block.0.to_string()).collect::<Vec<_>>().join(", "),
        chain_db = chain_db.to_string_lossy(),
        network = args.network,
    );

    let chain_store = RocksDBStore::open(&RocksDbConfig::new(chain_db))?;

    for PointOrHash(hash) in args.blocks {
        info!(cli::dev::chain::VALIDATION_CLEARED, header_hash = hash);
        chain_store.remove_block_valid(&hash)?;
    }

    Ok(())
}
