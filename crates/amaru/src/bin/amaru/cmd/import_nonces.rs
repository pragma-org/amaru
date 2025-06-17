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

use amaru_consensus::{consensus::store::ChainStore, Nonces};
use amaru_kernel::{
    default_chain_dir, network::NetworkName, EraHistory, Hash, Header, Nonce, Point,
};
use amaru_stores::rocksdb::consensus::RocksDBStore;
use clap::Parser;
use serde::{Deserialize, Deserializer};
use std::{error::Error, path::PathBuf};
use tracing::info;

#[derive(Debug, Parser)]
pub struct Args {
    /// Path of the consensus on-disk storage.
    #[arg(long, value_name = "DIR")]
    chain_dir: Option<PathBuf>,

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

#[derive(Debug, Deserialize)]
pub(crate) struct InitialNonces {
    #[serde(deserialize_with = "deserialize_point")]
    at: Point,
    active: Nonce,
    evolving: Nonce,
    candidate: Nonce,
    tail: Hash<32>,
}

fn deserialize_point<'de, D>(deserializer: D) -> Result<Point, D::Error>
where
    D: Deserializer<'de>,
{
    let buf = <&str>::deserialize(deserializer)?;
    super::parse_point(buf)
        .map_err(|e| serde::de::Error::custom(format!("cannot convert vector to nonce: {:?}", e)))
}

pub async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let chain_dir = args
        .chain_dir
        .unwrap_or_else(|| default_chain_dir(args.network).into());
    import_nonces(
        args.network.into(),
        &chain_dir,
        InitialNonces {
            at: args.at,
            active: args.active,
            evolving: args.evolving,
            candidate: args.candidate,
            tail: args.tail,
        },
    )
    .await
}

pub(crate) async fn import_nonces(
    era_history: &EraHistory,
    chain_db_path: &PathBuf,
    initial_nonce: InitialNonces,
) -> Result<(), Box<dyn Error>> {
    let mut db =
        Box::new(RocksDBStore::new(chain_db_path, era_history)?) as Box<dyn ChainStore<Header>>;

    let header_hash = Hash::from(&initial_nonce.at);

    info!(point.id = %header_hash, point.slot = %initial_nonce.at.slot_or_default(), "importing nonces");

    let epoch = {
        let slot = initial_nonce.at.slot_or_default();
        era_history.slot_to_epoch(slot)?
    };

    let nonces = Nonces {
        epoch,
        active: initial_nonce.active,
        evolving: initial_nonce.evolving,
        candidate: initial_nonce.candidate,
        tail: initial_nonce.tail,
    };

    db.put_nonces(&header_hash, &nonces)?;

    Ok(())
}
