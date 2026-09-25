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
use amaru_ouroboros::{BaseReadChainStore, ChildTipsMode, ReadChainStore};
use amaru_stores::rocksdb::{RocksDbConfig, consensus::RocksDBStore};
use clap::Parser;

use crate::cmd::PointOrHash;

#[derive(Debug, Parser)]
pub struct Args {
    /// The point or hash to walk forward from.
    #[arg(value_name = amaru::value_names::POINT_OR_HASH)]
    start: PointOrHash,

    #[command(flatten)]
    db_chain: amaru::args::DbChain,

    #[command(flatten)]
    network: amaru::args::Network,
}

pub(crate) fn runnable(args: Args) -> Runnable {
    Runnable::exit_on_signal(RuntimeKind::Simple, move || run(args))
}

#[expect(clippy::print_stdout)]
#[expect(clippy::unwrap_used)]
async fn run(args: Args) -> anyhow::Result<()> {
    let network = NetworkName::from(args.network);
    let db_chain = args.db_chain.into_path_buf(network);

    info!(cli::dev::chain::CHILDREN, db_chain = db_chain.to_string_lossy(), network, start = args.start.to_string());

    let db = RocksDBStore::open_for_readonly(&RocksDbConfig::new(db_chain))?;

    let children = db.child_tips(&args.start, ChildTipsMode::All);

    let mut count = 0u64;
    for child in &children {
        let (_header, valid) = db.load_header_with_validity(&child.hash()).unwrap();
        let has_block = db.has_block(&child.hash()).unwrap_or(false);
        let on_best_chain = db.is_on_best_chain((*child).into());

        println!(
            "{} height={} block={} valid={} best-chain={}",
            child,
            child.block_height(),
            if has_block { "yes" } else { "no" },
            valid_str(valid),
            if on_best_chain { "yes" } else { "no" },
        );
        count += 1;
    }

    println!("\n=> {count} children");

    Ok(())
}

fn valid_str(valid: Option<bool>) -> &'static str {
    match valid {
        Some(true) => "valid",
        Some(false) => "invalid",
        None => "-",
    }
}
