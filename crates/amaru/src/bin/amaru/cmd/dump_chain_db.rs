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

use amaru_kernel::network::NetworkName;
use amaru_kernel::string_utils::ListToString;
use amaru_kernel::{Header, to_cbor};
use amaru_ouroboros_traits::{ChainStore, IsHeader};
use amaru_stores::rocksdb::RocksDbConfig;
use amaru_stores::rocksdb::consensus::RocksDBStore;
use clap::{Parser, arg};
use std::fmt::Display;
use std::sync::Arc;
use std::{error::Error, path::PathBuf};

#[derive(Debug, Parser)]
pub struct Args {
    /// Network for which we are importing headers.
    ///
    /// Should be one of 'mainnet', 'preprod', 'preview' or 'testnet_<magic>' where
    /// `magic` is a 32-bits unsigned value denoting a particular testnet.
    #[arg(
        long,
        value_name = "NETWORK",
        env = "AMARU_NETWORK",
        default_value_t = NetworkName::Preprod,
    )]
    network: NetworkName,

    /// The path to the chain database to dump
    #[arg(long, value_name = "DIR", default_value = "chain.db")]
    chain_dir: PathBuf,
}

pub async fn run(args: Args) -> Result<(), Box<dyn Error>> {
    let chain_dir = args.chain_dir;
    let db: Arc<dyn ChainStore<Header>> =
        Arc::new(RocksDBStore::new(RocksDbConfig::new(chain_dir))?);

    print_iterator(
        "headers",
        db.load_headers().map(|header| {
            (
                format!("\n{}", header.hash()),
                hex::encode(to_cbor(&header)),
            )
        }),
    );
    print_iterator(
        "parent -> children relationships\n",
        db.load_parents_children()
            .map(|(parent, children)| (parent, children.list_to_string(", "))),
    );
    print_iterator(
        "nonces\n",
        db.load_nonces()
            .map(|(hash, nonces)| (hash, hex::encode(to_cbor(&nonces)))),
    );
    print_iterator(
        "blocks\n",
        db.load_blocks()
            .map(|(hash, block)| (hash, hex::encode(block.to_vec()))),
    );
    print_best_chain(db);
    Ok(())
}

#[expect(clippy::print_stdout)]
pub fn print_best_chain(db: Arc<dyn ChainStore<Header>>) {
    println!();
    let best_chain = db.retrieve_best_chain();
    println!(
        "The best chain is:\n  {}",
        best_chain.list_to_string("\n  ")
    );

    println!();
    println!("The best chain length is: {}", best_chain.len());
    println!("The best chain anchor is: {}", db.get_anchor_hash());
    println!("The best chain tip is: {}", db.get_best_chain_hash());
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
