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

use amaru::lifecycle::{Runnable, RuntimeKind};
use amaru_kernel::{IsHeader, NetworkName};
use amaru_observability::info;
use amaru_ouroboros::{BaseReadChainStore, DiagnosticChainStore, WriteChainStore};
use amaru_stores::rocksdb::{RocksDB, RocksDbConfig, consensus::RocksDBStore};
use anyhow::anyhow;
use clap::Parser;

#[derive(Debug, Parser)]
pub struct Args {
    #[command(flatten)]
    db_chain: amaru::args::DbChain,

    #[command(flatten)]
    db_ledger: amaru::args::DbLedger,

    #[command(flatten)]
    network: amaru::args::Network,
}

pub(crate) fn runnable(args: Args) -> Runnable {
    Runnable::exit_on_signal(RuntimeKind::Simple, move || run(args))
}

#[expect(clippy::print_stdout)]
async fn run(args: Args) -> anyhow::Result<()> {
    let network = NetworkName::from(args.network);
    let db_chain = args.db_chain.into_path_buf(network);
    let db_ledger = args.db_ledger.into_path_buf(network);

    info!(
        cli::dev::chain::PRUNE,
        db_chain = db_chain.to_string_lossy(),
        db_ledger = db_ledger.to_string_lossy(),
        network,
    );

    let era_history =
        network.as_era_history().ok_or_else(|| anyhow!("no era history available for network {network}"))?;

    let snapshots = RocksDB::snapshots(&db_ledger)?;
    if snapshots.is_empty() {
        anyhow::bail!("no ledger snapshots found; cannot determine safe pruning boundary");
    }

    let oldest_epoch = snapshots[0];
    let epoch_bounds = era_history.epoch_bounds(oldest_epoch)?;
    let boundary_slot = epoch_bounds.start;

    info!(
        cli::dev::chain::PRUNE_BOUNDARY,
        oldest_ledger_epoch = u64::from(oldest_epoch),
        boundary_slot = u64::from(boundary_slot)
    );

    let chain_store = RocksDBStore::open(&RocksDbConfig::new(db_chain))?;
    let anchor_hash = chain_store.get_anchor_hash();

    let tip_hash = chain_store.get_best_chain_hash();
    let chain: Vec<_> = chain_store.ancestors_hashes(&tip_hash).collect();

    let mut new_anchor_hash = None;
    let mut to_remove = Vec::new();
    for hash in &chain {
        let Some(header) = chain_store.load_header(hash) else {
            anyhow::bail!("header {hash} missing during prune walk; chain store may be corrupt");
        };
        if header.slot() >= boundary_slot {
            new_anchor_hash = Some(*hash);
        } else {
            to_remove.push(*hash);
        }
    }

    let Some(new_anchor) = new_anchor_hash else {
        anyhow::bail!(
            "every stored header is older than the prune boundary (slot {}); refusing to prune",
            u64::from(boundary_slot),
        );
    };

    for hash in &to_remove {
        chain_store.remove_header(hash)?;
    }

    if new_anchor != anchor_hash {
        let Some(point) = chain_store.load_point(&new_anchor) else {
            anyhow::bail!("header {new_anchor} missing while updating prune anchor");
        };
        chain_store.set_anchor_point(&point)?;
        info!(cli::dev::chain::ANCHOR_UPDATED, new_anchor);
    }

    let pruned = to_remove.len();
    println!(
        "Pruned {pruned} headers (boundary: slot {}, epoch {})",
        u64::from(boundary_slot),
        u64::from(oldest_epoch)
    );

    Ok(())
}
