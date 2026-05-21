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

use std::{
    error::Error,
    fs,
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};

use amaru::{
    DEFAULT_NETWORK, DEFAULT_PEER_ADDRESS,
    bootstrap::{BOOTSTRAP_HEADERS_PER_POINT, default_bootstrap_parent_points, fetch_headers_from_points},
};
use amaru_kernel::{BlockHeader, IsHeader, NetworkName, Point, from_cbor};
use clap::{ArgAction, Parser};
use tracing::info;

#[derive(Debug, Parser)]
pub struct Args {
    /// Path where to store fetched headers.
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::HEADERS_DIR,
        default_value = ".",
    )]
    headers_dir: PathBuf,

    /// Network to fetch chain headers from.
    #[arg(
        long,
        value_name = amaru::value_names::NETWORK,
        env = amaru::env_vars::NETWORK,
        default_value_t = DEFAULT_NETWORK,
    )]
    network: NetworkName,

    /// Parent point of the header to fetch.
    ///
    /// Headers are currently fetched using the (client) chain-sync protocol, which fetches data
    /// from a given point onward.
    ///
    /// To fetch point at slot `s`, we therefore need the parent at `s-1`.
    #[arg(
        long,
        value_name = amaru::value_names::POINT,
        env = amaru::env_vars::PARENT,
        action = ArgAction::Append,
        value_delimiter = ',',
        num_args(0..),
    )]
    parent: Vec<String>,

    /// Address of the node to connect to for retrieving chain data.
    ///
    /// The node should be accessible via the node-2-node protocol, which
    /// means the remote node should be running as a validator and not
    /// as a client node.
    #[arg(
        long,
        value_name = amaru::value_names::ENDPOINT,
        env = amaru::env_vars::PEER_ADDRESS,
        default_value = DEFAULT_PEER_ADDRESS,
    )]
    peer_address: String,
}

pub async fn run(args: Args) -> Result<(), Box<dyn Error>> {
    let network = args.network;

    info!(
        _command = "fetch-chain-headers",
        headers_dir = %args.headers_dir.to_string_lossy(),
        network = %args.network,
        parent = %args.parent.join(", "),
        peer_address= %args.peer_address,
        "running",
    );

    let points = if args.parent.is_empty() {
        default_bootstrap_parent_points(network)?
    } else {
        args.parent.iter().map(|point| Point::try_from(point.as_str())).collect::<Result<Vec<_>, _>>()?
    };

    fetch_headers_for_network(network, &args.headers_dir, &args.peer_address, &points).await?;

    Ok(())
}

async fn fetch_headers_for_network(
    network: NetworkName,
    headers_dir: &Path,
    peer_address: &str,
    points: &[Point],
) -> Result<(), Box<dyn Error>> {
    let headers = fetch_headers_from_points(peer_address, network, points, BOOTSTRAP_HEADERS_PER_POINT).await?;
    write_headers(headers_dir, headers)
}

fn write_headers(headers_dir: &Path, headers: Vec<Vec<u8>>) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(headers_dir)?;

    for header in headers {
        let block_header: BlockHeader = from_cbor(&header).ok_or("failed to decode fetched block header")?;
        let hash = block_header.hash();
        let slot = block_header.slot();
        let filename = format!("header.{}.{}.cbor", slot, hex::encode(hash));
        let filepath = headers_dir.join(filename);
        let mut file = File::create(&filepath)?;
        file.write_all(&header)?;
    }

    Ok(())
}
