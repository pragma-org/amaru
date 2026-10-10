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

    #[command(flatten)]
    network: amaru::args::Network,

    #[command(flatten, next_help_heading = "Storage options")]
    db_chain: amaru::args::DbChain,

    #[command(flatten, next_help_heading = "Storage options")]
    db_ledger: amaru::args::DbLedger,
}

pub(crate) fn runnable(args: Args) -> Runnable {
    Runnable::exit_on_signal(RuntimeKind::Simple, move || run(args))
}

/// Full recovery to the start of `epoch`: ledger snapshot reset + chain realign.
///
async fn run(args: Args) -> anyhow::Result<()> {
    let network = NetworkName::from(args.network);
    let db_chain = args.db_chain.into_path_buf(network);
    let db_ledger = args.db_ledger.into_path_buf(network);

    let mode = if args.immutable_tip { "immutable_tip" } else { "epoch" };

    if let Some(epoch) = args.epoch {
        reset_ledger_to_epoch(&db_ledger, epoch)?;
    }

    let ledger = ReadOnlyRocksDB::new(&RocksDbConfig::new(db_ledger.clone()))?;
    let tip = ledger.tip()?;

    let chain_store = RocksDBStore::open(&RocksDbConfig::new(db_chain.clone()))?;
    realign_chain_store_to(&chain_store, tip, ClearValidity::All)?;

    info!(
        cli::node::ROLLBACK,
        db_chain = db_chain.display().to_string(),
        db_ledger = db_ledger.display().to_string(),
        mode,
        network,
        anchor = @Some(chain_store.get_anchor_hash().to_string()),
        best_chain = @Some(chain_store.get_best_chain_hash().to_string()),
        epoch = @args.epoch.map(|e| e.as_u64()),
        ledger_tip = @Some(tip.to_string()),
    );

    Ok(())
}
