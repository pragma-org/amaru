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
    default_chain_dir, default_ledger_dir,
    lifecycle::{Runnable, RuntimeKind},
};
use amaru_kernel::{IsHeader, NetworkName};
use amaru_ouroboros::{BaseReadChainStore, DiagnosticChainStore, WriteChainStore};
use amaru_stores::rocksdb::{RocksDB, RocksDbConfig, consensus::RocksDBStore};
use clap::Parser;
use tracing::info;

#[derive(Debug, Parser)]
pub struct Args {
    /// The path to the chain database to prune.
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::CHAIN_DIR,
    )]
    chain_dir: Option<PathBuf>,

    /// The path to the ledger database (used to determine safe pruning boundary).
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::LEDGER_DIR,
    )]
    ledger_dir: Option<PathBuf>,

    /// Network of the underlying databases.
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

#[expect(clippy::print_stdout)]
async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let chain_dir = args.chain_dir.unwrap_or_else(|| default_chain_dir(args.network).into());
    let ledger_dir = args.ledger_dir.unwrap_or_else(|| default_ledger_dir(args.network).into());

    info!(
        _command = "dev chain prune",
        chain_dir = %chain_dir.to_string_lossy(),
        ledger_dir = %ledger_dir.to_string_lossy(),
        network = %args.network,
        "running",
    );

    let era_history = args
        .network
        .as_era_history()
        .ok_or_else(|| format!("no era history available for network {}", args.network))?;

    let snapshots = RocksDB::snapshots(&ledger_dir)?;
    if snapshots.is_empty() {
        return Err("no ledger snapshots found; cannot determine safe pruning boundary".into());
    }

    let oldest_epoch = snapshots[0];
    let epoch_bounds = era_history.epoch_bounds(oldest_epoch)?;
    let boundary_slot = epoch_bounds.start;

    info!(
        oldest_ledger_epoch = u64::from(oldest_epoch),
        boundary_slot = u64::from(boundary_slot),
        "determined safe pruning boundary",
    );

    let chain_store = RocksDBStore::open(&RocksDbConfig::new(chain_dir))?;
    let anchor_hash = chain_store.get_anchor_hash();

    let tip_hash = chain_store.get_best_chain_hash();
    let chain: Vec<_> = chain_store.ancestors_hashes(&tip_hash).collect();

    let mut new_anchor_hash = None;
    let mut to_remove = Vec::new();
    for hash in &chain {
        let Some(header) = chain_store.load_header(hash) else {
            return Err(format!("header {hash} missing during prune walk; chain store may be corrupt").into());
        };
        if header.slot() >= boundary_slot {
            new_anchor_hash = Some(*hash);
        } else {
            to_remove.push(*hash);
        }
    }

    let Some(new_anchor) = new_anchor_hash else {
        return Err(format!(
            "every stored header is older than the prune boundary (slot {}); refusing to prune",
            u64::from(boundary_slot),
        )
        .into());
    };

    for hash in &to_remove {
        chain_store.remove_header(hash)?;
    }

    if new_anchor != anchor_hash {
        chain_store.set_anchor_hash(&new_anchor)?;
        info!(%new_anchor, "updated anchor hash");
    }

    let pruned = to_remove.len();
    println!(
        "Pruned {pruned} headers (boundary: slot {}, epoch {})",
        u64::from(boundary_slot),
        u64::from(oldest_epoch)
    );

    Ok(())
}
