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

use amaru_kernel::{Header, default_chain_dir, from_cbor, network::NetworkName};
use amaru_ouroboros_traits::ChainStore;
use amaru_stores::rocksdb::consensus::RocksDBStore;
use clap::Parser;
use gasket::framework::*;
use std::{
    error::Error,
    path::{Path, PathBuf},
};
use tokio::{fs::File, io::AsyncReadExt};

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

    /// Path of the consensus on-disk storage.
    ///
    /// This is the directory where data will be stored. The directory and any intermediate
    /// paths will be created if they do not exist.
    #[arg(
        long,
        value_name = "DIR",
        env = "AMARU_CHAIN_DIR",
        verbatim_doc_comment
    )]
    chain_dir: Option<PathBuf>,

    /// Path to directory containing per-network configuration files.
    ///
    /// This path will be used as a prefix to resolve per-network configuration files
    /// needed for importing headers. Given a source directory `data`, and a
    /// a network name of `preview`, the expected layout for header files would be:
    ///
    /// `data/preview/headers/header.*.*.cbor`
    #[arg(
        long,
        value_name = "DIRECTORY",
        default_value = "data",
        env = "AMARU_CONFIG_DIR",
        verbatim_doc_comment
    )]
    config_dir: PathBuf,
}

pub async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let network = args.network;
    let network_dir = args.config_dir.join(&*network.to_string());
    let chain_dir = args
        .chain_dir
        .unwrap_or_else(|| default_chain_dir(args.network).into());
    import_headers_for_network(args.network, &network_dir, &chain_dir).await
}

#[allow(clippy::unwrap_used)]
pub(crate) async fn import_headers_for_network(
    network: NetworkName,
    config_dir: &Path,
    chain_dir: &PathBuf,
) -> Result<(), Box<dyn Error>> {
    let era_history = network.into();
    let db = RocksDBStore::new(chain_dir, era_history)?;

    for entry in std::fs::read_dir(config_dir.join("headers"))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file()
            && let Some(filename) = path.file_name().and_then(|f| f.to_str())
            && filename.starts_with("header.")
            && filename.ends_with(".cbor")
        {
            let mut file = File::open(&path).await
                .inspect_err(|reason| tracing::error!(file = %path.display(), reason = %reason, "Failed to open header file"))
                .map_err(|_| WorkerError::Panic)?;
            let mut cbor_data = Vec::new();
            file.read_to_end(&mut cbor_data).await
                .inspect_err(|reason| tracing::error!(file = %path.display(), reason = %reason, "Failed to read header file"))
                .map_err(|_| WorkerError::Panic)?;
            let header_from_file: Header = from_cbor(&cbor_data).unwrap();
            db.store_header(&header_from_file)
                .map_err(|_| WorkerError::Panic)?;
        }
    }

    Ok(())
}
