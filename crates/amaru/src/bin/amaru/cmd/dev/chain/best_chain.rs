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
use amaru_consensus::effects::find_best_candidate;
use amaru_kernel::{NetworkName, utils::string::ListToString};
use amaru_observability::info;
use amaru_ouroboros::{BaseReadChainStore, DiagnosticChainStore};
use amaru_stores::rocksdb::{RocksDbConfig, consensus::RocksDBStore};
use clap::Parser;

#[derive(Debug, Parser)]
pub struct Args {
    #[command(flatten)]
    db_chain: amaru::args::DbChain,

    #[command(flatten)]
    network: amaru::args::Network,
}

pub(crate) fn runnable(args: Args) -> Runnable {
    Runnable::exit_on_signal(RuntimeKind::Simple, move || run(args))
}

#[expect(clippy::print_stdout)]
async fn run(args: Args) -> anyhow::Result<()> {
    let db_chain = args.db_chain.into_path_buf(args.network);

    info!(
        cli::dev::chain::BEST_CHAIN,
        db_chain = db_chain.to_string_lossy(),
        network = NetworkName::from(args.network)
    );

    let db = RocksDBStore::open_for_readonly(&RocksDbConfig::new(db_chain))?;

    let best_chain = db.retrieve_best_chain();
    let anchor = db.get_anchor_hash();
    let best_tip = db.get_best_chain_tip();

    println!("Anchor:           {anchor}");
    println!("Best tip (stored): {}", best_tip);
    println!("Best chain length: {}", best_chain.len());

    match find_best_candidate(&db) {
        Ok(candidate) => println!("Best tip candidate (computed): {candidate}"),
        Err(e) => println!("Best tip candidate (computed): error - {e}"),
    }

    if best_chain.len() <= 20 {
        println!("\nBest chain:\n  {}", best_chain.list_to_string("\n  "));
    }

    Ok(())
}
