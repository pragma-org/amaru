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

use amaru::{
    bootstrap::import_headers_for_network, bootstrap_config_dir, default_chain_dir,
    get_bootstrap_headers,
};
use amaru_kernel::network::NetworkName;
use clap::Parser;
use std::path::PathBuf;
use tracing::info;

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
        default_value_t = super::DEFAULT_NETWORK,
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
        value_name = "DIR",
        env = "AMARU_CONFIG_DIR",
        verbatim_doc_comment
    )]
    config_dir: Option<PathBuf>,
}

pub async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let network = args.network;
    let config_dir = args
        .config_dir
        .unwrap_or_else(|| bootstrap_config_dir(network));
    let chain_dir = args
        .chain_dir
        .unwrap_or_else(|| default_chain_dir(args.network).into());

    info!(%network, chain_dir=%chain_dir.to_string_lossy(), config_dir=%config_dir.to_string_lossy(),
          "Running command import-headers",
    );

    let headers = get_bootstrap_headers(network)?.collect::<Vec<_>>();
    import_headers_for_network(&chain_dir, headers).await
}
