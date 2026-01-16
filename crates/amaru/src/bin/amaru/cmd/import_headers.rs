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
    DEFAULT_NETWORK, bootstrap::import_headers_for_network, default_chain_dir,
    get_bootstrap_headers,
};
use amaru_kernel::network::NetworkName;
use clap::{ArgAction, Parser};
use std::{fs, path::PathBuf};
use tracing::info;

#[derive(Debug, Parser)]
pub struct Args {
    /// Path of the consensus on-disk storage.
    ///
    /// This is the directory where data will be stored. The directory and any intermediate
    /// paths will be created if they do not exist.
    ///
    /// Defaults to ./chain.<NETWORK>.db when unspecified.
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::CHAIN_DIR,
        verbatim_doc_comment
    )]
    chain_dir: Option<PathBuf>,

    /// Path to a header CBOR file to import. Can be repeated for multiple files.
    ///
    /// The expected format is a raw CBOR serialised header, following the usual network's
    /// serialisation codec rules for block headers.
    #[arg(
        long,
        value_name = amaru::value_names::FILEPATH,
        env = amaru::env_vars::HEADER_FILE,
        action = ArgAction::Append,
        value_delimiter = ',',
        num_args(0..),
    )]
    header_file: Vec<PathBuf>,

    /// Network for which we are importing headers.
    #[arg(
        long,
        value_name = amaru::value_names::NETWORK,
        env = amaru::env_vars::NETWORK,
        default_value_t = DEFAULT_NETWORK,
    )]
    network: NetworkName,
}

pub async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let network = args.network;

    let chain_dir = args
        .chain_dir
        .unwrap_or_else(|| default_chain_dir(args.network).into());

    info!(
        _command = "import-headers",
        chain_dir = %chain_dir.to_string_lossy(),
        header_files = %args
            .header_file
            .iter()
            .map(|s| s.to_string_lossy())
            .collect::<Vec<_>>().join(", "),
        %network,
        "running",
    );

    let headers = if args.header_file.is_empty() {
        get_bootstrap_headers(network)?.collect::<Vec<_>>()
    } else {
        args.header_file
            .iter()
            .map(fs::read)
            .collect::<Result<_, _>>()?
    };

    import_headers_for_network(&chain_dir, headers).await
}
