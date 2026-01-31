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

use amaru::{DEFAULT_NETWORK, default_chain_dir};
use amaru_consensus::{DiagnosticChainStore, ReadOnlyChainStore};
use amaru_kernel::{BlockHeader, IsHeader, NetworkName, to_cbor, utils::string::ListToString};
use amaru_stores::rocksdb::{
    RocksDbConfig,
    consensus::{ReadOnlyChainDB, RocksDBStore},
};
use clap::Parser;
use std::{error::Error, fmt::Display, path::PathBuf};
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
}

pub async fn run(args: Args) -> Result<(), Box<dyn Error>> {
    let chain_dir = args
        .chain_dir
        .unwrap_or_else(|| default_chain_dir(args.network).into());

    info!(
        _command = "dump-chain-db",
        chain_dir = %chain_dir.to_string_lossy(),
        network = %args.network,
        "running",
    );

    let db: ReadOnlyChainDB = RocksDBStore::open_for_readonly(&RocksDbConfig::new(chain_dir))?;

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
pub fn print_best_chain(db: ReadOnlyChainDB) {
    println!();
    let best_chain = <ReadOnlyChainDB as ReadOnlyChainStore<BlockHeader>>::retrieve_best_chain(&db);
    println!(
        "The best chain is:\n  {}",
        best_chain.list_to_string("\n  ")
    );

    println!();
    println!("The best chain length is: {}", best_chain.len());
    println!(
        "The best chain anchor is: {}",
        <ReadOnlyChainDB as ReadOnlyChainStore<BlockHeader>>::get_anchor_hash(&db)
    );
    println!(
        "The best chain tip is: {}",
        <ReadOnlyChainDB as ReadOnlyChainStore<BlockHeader>>::get_best_chain_hash(&db)
    );
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
