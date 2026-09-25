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
use amaru_kernel::NetworkName;
use amaru_observability::info;
use amaru_ouroboros::BaseReadChainStore;
use amaru_stores::rocksdb::{RocksDbConfig, consensus::RocksDBStore};
use clap::Parser;

use crate::cmd::PointOrHash;

#[derive(Debug, Parser)]
pub struct Args {
    /// The block hash or point to look up nonces for.
    #[arg(value_name = amaru::value_names::POINT_OR_HASH)]
    block: PointOrHash,

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
    let network = NetworkName::from(args.network);
    let db_chain = args.db_chain.into_path_buf(network);

    info!(
        cli::dev::ledger::nonces::GET,
        block = args.block.to_string(),
        db_chain = db_chain.to_string_lossy(),
        network,
    );

    let db = RocksDBStore::open_for_readonly(&RocksDbConfig::new(db_chain))?;

    match db.get_nonces(&args.block) {
        Some(nonces) => {
            println!("Nonces for {}:", *args.block);
            println!("  active:    {}", nonces.active);
            println!("  evolving:  {}", nonces.evolving);
            println!("  candidate: {}", nonces.candidate);
            println!("  tail:      {}", nonces.tail);
            println!("  epoch:     {}", nonces.epoch);
        }
        None => {
            println!("No nonces found for {}", *args.block);
        }
    }

    Ok(())
}
