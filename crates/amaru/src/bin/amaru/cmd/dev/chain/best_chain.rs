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
use amaru_consensus::effects::find_best_candidate;
use amaru_kernel::{NetworkName, utils::string::ListToString};
use amaru_observability::info;
use amaru_ouroboros::{BaseReadChainStore, DiagnosticChainStore};
use amaru_stores::rocksdb::{RocksDbConfig, consensus::RocksDBStore};
use clap::Parser;

#[derive(Debug, Parser)]
pub struct Args {
    /// The path to the chain database.
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
    )]
    network: NetworkName,
}

pub(crate) fn runnable(args: Args) -> Runnable {
    Runnable::exit_on_signal(RuntimeKind::Simple, move || run(args))
}

#[expect(clippy::print_stdout)]
async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let chain_dir = args.chain_dir.unwrap_or_else(|| default_chain_dir(args.network).into());

    info!(
        cli::dev::RUN,
        command = "dev chain best-chain",
        network = args.network,
        chain_dir = chain_dir.to_string_lossy()
    );

    let db = RocksDBStore::open_for_readonly(&RocksDbConfig::new(chain_dir))?;

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
