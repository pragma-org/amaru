// Copyright 2025 PRAGMA
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

use std::{error::Error, fmt::Display, path::PathBuf};

use amaru::{DEFAULT_NETWORK, default_chain_dir};
use amaru_kernel::{BlockHeader, IsHeader, NetworkName, Point, to_cbor, utils::string::ListToString};
use amaru_ouroboros::{DiagnosticChainStore, ReadOnlyChainStore};
use amaru_stores::rocksdb::{RocksDbConfig, consensus::RocksDBStore};
use clap::Parser;
use tracing::info;

#[derive(Debug, Parser)]
pub struct Args {
    /// The path to the chain database to dump.
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::CHAIN_DIR
    )]
    chain_dir: Option<PathBuf>,

    /// Network for which we are importing headers.
    #[arg(
        long,
        value_name = amaru::value_names::NETWORK,
        env = amaru::env_vars::NETWORK,
        default_value_t = DEFAULT_NETWORK,
    )]
    network: NetworkName,

    #[arg(short = 'H', long)]
    headers: bool,

    #[arg(short, long)]
    parents_children: bool,

    #[arg(short, long)]
    nonces: bool,

    #[arg(short = 'B', long)]
    blocks: bool,

    #[arg(short, long)]
    best_chain: bool,

    #[arg(short, long)]
    ancestors: Option<Point>,
}

pub async fn run(args: Args) -> Result<(), Box<dyn Error>> {
    let chain_dir = args.chain_dir.unwrap_or_else(|| default_chain_dir(args.network).into());

    info!(
        _command = "dump-chain-db",
        chain_dir = %chain_dir.to_string_lossy(),
        network = %args.network,
        "running",
    );

    let db = RocksDBStore::open_for_readonly(&RocksDbConfig::new(chain_dir))?;

    if args.headers {
        print_iterator(
            "headers",
            db.load_headers().map(|header| (format!("\n{}", header.hash()), hex::encode(to_cbor(&header)))),
        );
    }
    if args.parents_children {
        print_iterator(
            "parent -> children relationships\n",
            db.load_parents_children().map(|(parent, children)| (parent, children.list_to_string(", "))),
        );
    }
    if args.nonces {
        print_iterator("nonces\n", db.load_nonces().map(|(hash, nonces)| (hash, hex::encode(to_cbor(&nonces)))));
    }
    if args.blocks {
        print_iterator("blocks\n", db.load_blocks().map(|(hash, block)| (hash, hex::encode(block.to_vec()))));
    }
    if args.best_chain {
        print_best_chain(&db);
    }
    if let Some(ancestors) = args.ancestors {
        print_ancestors(&db, ancestors);
    }
    Ok(())
}

#[expect(clippy::print_stdout)]
pub fn print_best_chain(db: &impl ReadOnlyChainStore<BlockHeader>) {
    println!();
    let best_chain = db.retrieve_best_chain();
    println!("The best chain is:\n  {}", best_chain.list_to_string("\n  "));

    println!();
    println!("The best chain length is: {}", best_chain.len());
    println!("The best chain anchor is: {}", db.get_anchor_hash());
    println!("The best chain tip is: {}", db.load_tip(&db.get_best_chain_hash()).unwrap().point());
}

#[expect(clippy::print_stdout)]
pub fn print_iterator<K: Display, V: Display>(title: &str, iterator: impl Iterator<Item = (K, V)>) {
    println!("\n{}", title.to_ascii_uppercase());
    let mut count = 0;
    for (k, v) in iterator {
        println!("{}: {}", k, v);
        count += 1;
    }
    // remove newlines
    let mut lower = title.to_lowercase().clone();
    lower.retain(|c| c != '\n');
    println!("=> Found {} {}", count, lower);
}

#[expect(clippy::print_stdout)]
pub fn print_ancestors(db: &impl ReadOnlyChainStore<BlockHeader>, point: Point) {
    println!();
    let ancestors = db.ancestors_with_validity(point.hash());
    println!("The ancestors of {} are:", point);
    let mut count = 0;
    println!();
    for (ancestor, valid) in ancestors {
        let valid_str = match valid {
            Some(true) => "valid",
            Some(false) => "invalid",
            None => "-",
        };
        println!("{} {}", ancestor.point(), valid_str);
        count += 1;
    }
    println!();
    println!("The ancestors length is: {}", count);
}
