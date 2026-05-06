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
use amaru_kernel::{BlockHeader, IsHeader, NetworkName, Point};
use amaru_ouroboros::{ChainStore, ChildTipsMode};
use amaru_stores::rocksdb::{RocksDbConfig, consensus::RocksDBStore};

#[derive(Debug, clap::Parser)]
pub struct Args {
    /// The point from which onward to remove the chain
    #[arg(
        long,
        value_name = amaru::value_names::POINT,
    )]
    from_point: Point,

    /// Remove only blocks
    ///
    /// This also removes the validation status of the blocks.
    #[arg(long, default_value_t = false)]
    only_blocks: bool,

    /// Remove only validation results
    #[arg(long, default_value_t = false)]
    only_validation_results: bool,

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
    let Args { from_point, only_blocks, only_validation_results, chain_dir, network } = args;
    let chain_dir = chain_dir.unwrap_or_else(|| default_chain_dir(network).into());

    if only_blocks && only_validation_results {
        return Err("cannot combine both --only-blocks and --only-validation-results".into());
    }

    tracing::info!(
        _command = "remove-chain",
        chain_dir = %chain_dir.to_string_lossy(),
         %network, %from_point, %only_blocks, %only_validation_results,
        "running",
    );

    let rocks_db = RocksDBStore::open(&RocksDbConfig::new(chain_dir))?;
    let chain_store: &dyn ChainStore<BlockHeader> = &rocks_db;

    let points = chain_store.child_tips(&from_point.hash(), ChildTipsMode::All).collect::<Vec<_>>();
    tracing::info!(points = %points.len(), "points to remove");

    let best_chain_hash = chain_store.get_best_chain_hash();
    if points.iter().any(|p| p.hash() == best_chain_hash) || chain_store.load_header(&best_chain_hash).is_none() {
        tracing::warn!("moving back best chain hash");
        let mut current = chain_store.load_header(&from_point.hash()).unwrap();
        loop {
            let Some(parent) = current.parent().and_then(|h| chain_store.load_header(&h)) else {
                tracing::error!("no parent found for {}", current.hash());
                return Err("no parent found for best chain hash".into());
            };
            if chain_store.load_from_best_chain(&parent.point()).is_some() {
                chain_store.set_best_chain_hash(&parent.hash())?;
                break;
            }
            current = parent;
        }
    }

    for point in points {
        tracing::info!(point = %point.point(), "removing point");
        if only_validation_results {
            rocks_db.remove_block_valid(&point.hash())?;
        } else if only_blocks {
            rocks_db.remove_block(&point.hash())?;
        } else {
            rocks_db.remove_header::<BlockHeader>(&point.hash())?;
        }
    }

    Ok(())
}
