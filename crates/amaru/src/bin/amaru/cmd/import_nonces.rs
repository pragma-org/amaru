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
    bootstrap::{BootstrapError, import_nonces_from_file},
    default_chain_dir, get_bootstrap_file,
};
use amaru_kernel::network::NetworkName;
use clap::Parser;
use std::path::PathBuf;
use tracing::info;

#[derive(Debug, Parser)]
pub struct Args {
    /// Path of the consensus on-disk storage.
    #[arg(long, value_name = "DIR", env = "AMARU_CHAIN_DIR")]
    chain_dir: Option<PathBuf>,

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

pub async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let network = args.network;
    let chain_dir = args
        .chain_dir
        .unwrap_or_else(|| default_chain_dir(network).into());

    info!(network = %network, chain_dir=%chain_dir.to_string_lossy(),
          "Running command import-nonces",
    );

    let nonces_file_name = "nonces.json";
    let nonces_file = get_bootstrap_file(network, nonces_file_name)?
        .ok_or(BootstrapError::MissingConfigFile(nonces_file_name.into()))?;

    // FIXME: import nonces function takes an EraHistory which we
    // construct from NetworkName. In the case of testnets this can be
    // problematic hence why we have started writing and reading such
    // files in import_ledger_state.
    import_nonces_from_file(args.network.into(), &chain_dir, &nonces_file).await
}
