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
use amaru_kernel::{Epoch, NetworkName};
use amaru_ledger::store::ReadStore;
use amaru_node::{ClearValidity, realign_chain_store_to, reset_ledger_to_epoch};
use amaru_observability::info;
use amaru_ouroboros::BaseReadChainStore;
use amaru_stores::rocksdb::{ReadOnlyRocksDB, RocksDbConfig, consensus::RocksDBStore};
use clap::{ArgGroup, Parser};

#[derive(Debug, Parser)]
#[command(group(
    ArgGroup::new("target")
        .required(true)
        .args(["immutable_tip", "epoch"])
))]
pub struct Args {
    /// Roll the chain store back to the ledger's immutable tip.
    ///
    /// Does not modify the ledger database.
    #[arg(long)]
    immutable_tip: bool,

    /// Roll the ledger back to the beginning of this epoch, then realign the chain store to the
    /// resulting ledger tip.
    #[arg(long, value_name = amaru::value_names::UINT, env = amaru::env_vars::EPOCH)]
    epoch: Option<Epoch>,

    /// Path of the chain on-disk storage.
    ///
    /// Defaults to ./chain.<NETWORK>.db when unspecified.
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::CHAIN_DIR,
    )]
    chain_dir: Option<PathBuf>,

    /// Path of the ledger on-disk storage.
    ///
    /// Defaults to ./ledger.<NETWORK>.db when unspecified.
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::LEDGER_DIR,
    )]
    ledger_dir: Option<PathBuf>,

    /// Network whose node databases should be rolled back.
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

/// Full recovery to the start of `epoch`: ledger snapshot reset + chain realign.
///
/// Used by `amaru node rollback --epoch` and the legacy `reset-to-epoch` alias.
pub(crate) fn runnable_epoch(
    network: NetworkName,
    epoch: Epoch,
    ledger_dir: Option<PathBuf>,
    chain_dir: Option<PathBuf>,
) -> Runnable {
    runnable(Args { immutable_tip: false, epoch: Some(epoch), chain_dir, ledger_dir, network })
}

async fn run(args: Args) -> anyhow::Result<()> {
    let network = args.network;
    let chain_dir = args.chain_dir.unwrap_or_else(|| default_chain_dir(network).into());
    let ledger_dir = args.ledger_dir.unwrap_or_else(|| default_ledger_dir(network).into());

    let mode = if args.immutable_tip { "immutable_tip" } else { "epoch" };

    if let Some(epoch) = args.epoch {
        reset_ledger_to_epoch(&ledger_dir, epoch)?;
    }

    let ledger = ReadOnlyRocksDB::new(&RocksDbConfig::new(ledger_dir.clone()))?;
    let tip = ledger.tip()?;

    let chain_store = RocksDBStore::open(&RocksDbConfig::new(chain_dir.clone()))?;
    realign_chain_store_to(&chain_store, tip, ClearValidity::All)?;

    info!(
        cli::node::ROLLBACK,
        chain_dir = chain_dir.display().to_string(),
        ledger_dir = ledger_dir.display().to_string(),
        network = network,
        mode = mode,
        epoch = @args.epoch.map(|e| e.as_u64()),
        ledger_tip = @Some(tip.to_string()),
        best_chain = @Some(chain_store.get_best_chain_hash().to_string()),
        anchor = @Some(chain_store.get_anchor_hash().to_string()),
    );

    Ok(())
}
