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

use amaru_consensus::consensus::store::{rocksdb::RocksDBStore, ChainStore, Nonces};
use amaru_kernel::{network::NetworkName, Hash, Header, Nonce, Point};
use clap::Parser;
use std::path::PathBuf;
use tracing::info;

#[derive(Debug, Parser)]
pub struct Args {
    /// Path of the consensus on-disk storage.
    #[arg(long, value_name = "DIR", default_value = super::DEFAULT_CHAIN_DB_DIR)]
    chain_dir: PathBuf,

    /// Point for which nonces data is imported.
    #[arg(long, value_name = "POINT", value_parser = super::parse_point)]
    at: Point,

    /// Epoch active nonce at the specified point.
    #[arg(long, value_name = "NONCE", value_parser = super::parse_nonce)]
    active: Nonce,

    /// Next epoch's candidate nonce
    #[arg(long, value_name = "NONCE", value_parser = super::parse_nonce)]
    candidate: Nonce,

    /// Protocol evolving nonce vaue at the specified point.
    #[arg(long, value_name = "NONCE", value_parser = super::parse_nonce)]
    evolving: Nonce,

    /// The previous epoch last block header hash
    #[arg(long, value_name = "HEADER-HASH", value_parser = super::parse_nonce)]
    tail: Hash<32>,

    /// Network the nonces are imported for
    ///
    /// Should be one of 'mainnet', 'preprod', 'preview' or 'testnet:<magic>' where
    /// `magic` is a 32-bits unsigned value denoting a particular testnet.
    #[arg(
        long,
        value_name = "NETWORK",
        default_value_t = NetworkName::Preprod,
    )]
    network: NetworkName,
}

pub async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let era_history = args.network.into();
    let mut db =
        Box::new(RocksDBStore::new(args.chain_dir, era_history)?) as Box<dyn ChainStore<Header>>;

    let header_hash = Hash::from(&args.at);

    info!(point.id = %header_hash, point.slot = args.at.slot_or_default(), "importing nonces");

    let nonces = Nonces {
        epoch: {
            let slot = args.at.slot_or_default();
            // FIXME: currently hardwired to preprod network
            era_history.slot_to_epoch(From::from(slot))?
        },
        active: args.active,
        evolving: args.evolving,
        candidate: args.candidate,
        tail: args.tail,
    };

    db.put_nonces(&header_hash, nonces)?;

    Ok(())
}
