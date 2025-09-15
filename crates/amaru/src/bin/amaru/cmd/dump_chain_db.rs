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
use amaru_stores::rocksdb::consensus::{BLOCK_PREFIX, NONCES_PREFIX, RocksDBStore};
use clap::{Parser, arg};
use std::{error::Error, path::PathBuf};

#[derive(Debug, Parser)]
pub struct Args {
    /// Network for which we are importing headers.
    ///
    /// Should be one of 'mainnet', 'preprod', 'preview' or 'testnet:<magic>' where
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
    let era_history = args.network.into();
    let db = RocksDBStore::new(&chain_dir, era_history)?;
    let mut count: usize = 0;

    let transaction = db.create_transaction();
    println!("dumping headers");
    for item in transaction.prefix_iterator("") {
        match item {
            Ok((k, v)) => {
                count += 1;
                println!("{}:{}", hex::encode(k), hex::encode(v))
            }
            Err(err) => panic!("failed iterating over DB {}", err),
        }
    }
    println!("dumped {} headers", count);

    println!("dumping nonces");
    count = 0;
    for item in transaction.prefix_iterator(NONCES_PREFIX) {
        match item {
            Ok((k, v)) => {
                count += 1;
                println!("{}:{}", hex::encode(&k[5..]), hex::encode(v))
            }
            Err(err) => panic!("failed iterating over DB {}", err),
        }
    }
    println!("dumped {} nonces", count);

    println!("dumping blocks");
    count = 0;
    for item in transaction.prefix_iterator(BLOCK_PREFIX) {
        match item {
            Ok((k, v)) => {
                count += 1;
                println!("{}:{}", hex::encode(&k[5..]), hex::encode(v))
            }
            Err(err) => panic!("failed iterating over DB {}", err),
        }
    }
    println!("dumped {} blocks", count);

    transaction.commit()?;

    Ok(())
}
