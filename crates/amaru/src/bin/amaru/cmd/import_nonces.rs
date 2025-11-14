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

use amaru_kernel::{BlockHeader, EraHistory, Hash, HeaderHash, Nonce, Point, network::NetworkName};
use amaru_ouroboros_traits::{ChainStore, Nonces};
use amaru_stores::rocksdb::{RocksDbConfig, consensus::RocksDBStore};
use clap::Parser;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{error::Error, path::PathBuf};
use tracing::info;

use crate::cmd::{default_chain_dir, default_data_dir};

#[derive(Debug, Parser)]
pub struct Args {
    /// Path of the consensus on-disk storage.
    #[arg(long, value_name = "DIR", env = "AMARU_CHAIN_DIR")]
    chain_dir: Option<PathBuf>,

    /// JSON-formatted file with nonces details.
    #[arg(long, value_name = "FILE", env = "AMARU_NONCES_FILE")]
    nonces_file: Option<PathBuf>,

    /// Network the nonces are imported for
    ///
    /// Should be one of 'mainnet', 'preprod', 'preview' or 'testnet_<magic>' where
    /// `magic` is a 32-bits unsigned value denoting a particular testnet.
    #[arg(
        long,
        value_name = "NETWORK",
        env = "AMARU_NETWORK",
        default_value_t = super::DEFAULT_NETWORK,
    )]
    network: NetworkName,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct InitialNonces {
    #[serde(
        serialize_with = "serialize_point",
        deserialize_with = "deserialize_point"
    )]
    pub at: Point,
    pub active: Nonce,
    pub evolving: Nonce,
    pub candidate: Nonce,
    pub tail: HeaderHash,
}

fn deserialize_point<'de, D>(deserializer: D) -> Result<Point, D::Error>
where
    D: Deserializer<'de>,
{
    let buf = <&str>::deserialize(deserializer)?;
    Point::try_from(buf)
        .map_err(|e| serde::de::Error::custom(format!("cannot convert vector to nonce: {:?}", e)))
}

fn serialize_point<S: Serializer>(point: &Point, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&point.to_string())
}

pub async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let nonces_file = args
        .nonces_file
        .unwrap_or_else(|| default_data_dir(args.network).into())
        .join("nonces.json");

    let chain_dir = args
        .chain_dir
        .unwrap_or_else(|| default_chain_dir(args.network).into());

    // FIXME: import nonces function takes an EraHistory which we
    // construct from NetworkName. In the case of testnets this can be
    // problematic hence why we have started writing and reading such
    // files in import_ledger_state.
    import_nonces_from_file(args.network.into(), &nonces_file, &chain_dir).await
}

pub(crate) async fn import_nonces(
    era_history: &EraHistory,
    chain_db_path: &PathBuf,
    initial_nonce: InitialNonces,
) -> Result<(), Box<dyn Error>> {
    let db = Box::new(RocksDBStore::open_and_migrate(RocksDbConfig::new(
        chain_db_path.into(),
    ))?) as Box<dyn ChainStore<BlockHeader>>;

    let header_hash = Hash::from(&initial_nonce.at);

    info!(point.id = %header_hash, point.slot = %initial_nonce.at.slot_or_default(), "importing nonces");

    let epoch = {
        let slot = initial_nonce.at.slot_or_default();
        // NOTE: The slot definitely exists and is within one of the known eras.
        era_history.slot_to_epoch_unchecked_horizon(slot)?
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

pub async fn import_nonces_from_file(
    era_history: &EraHistory,
    nonces_file: &PathBuf,
    chain_dir: &PathBuf,
) -> Result<(), Box<dyn Error>> {
    let content = tokio::fs::read_to_string(nonces_file).await?;
    let initial_nonces: InitialNonces = serde_json::from_str(&content)?;
    import_nonces(era_history, chain_dir, initial_nonces).await?;
    Ok(())
}
