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

use std::path::Path;
use std::{error::Error, io, path::PathBuf};

use amaru::snapshots_dir;
use amaru_kernel::network::NetworkName;
use amaru_kernel::{default_chain_dir, default_ledger_dir};
use async_compression::tokio::bufread::GzipDecoder;
use clap::{arg, Parser};
use futures_util::TryStreamExt;
use serde::Deserialize;
use tokio::fs::{self, File};
use tokio::io::BufReader;
use tokio_util::io::StreamReader;
use tracing::info;

use crate::cmd::DEFAULT_NETWORK;

use super::import_headers::import_headers;
use super::import_ledger_state::import_all_from_directory;
use super::import_nonces::{import_nonces, InitialNonces};

#[derive(Debug, Parser)]
pub struct Args {
    /// Path of the ledger on-disk storage.
    #[arg(long, value_name = "DIR")]
    ledger_dir: Option<PathBuf>,

    /// Path of the chain on-disk storage.
    #[arg(long, value_name = "DIR")]
    chain_dir: Option<PathBuf>,

    /// Network to bootstrap the node for.
    ///
    /// Should be one of 'mainnet', 'preprod', 'preview' or 'testnet:<magic>' where
    /// `magic` is a 32-bits unsigned value denoting a particular testnet.
    #[arg(
        long,
        value_name = "NETWORK",
        default_value_t = DEFAULT_NETWORK,
    )]
    network: NetworkName,

    /// Path to directory containing per-network bootstrap configuration files.
    ///
    /// This path will be used as a prefix to resolve per-network configuration files
    /// needed for bootstrapping. Given a source directory `data`, and a
    /// a network name of `preview`, the expected layout for configuration files would be:
    ///
    /// * `data/preview/snapshots.json`: a list of `Snapshot` vaalues,
    /// * `data/preview/nonces.json`: a list of `InitialNonces` values,
    /// * `data/preview/headers.json`: a list of `Point`s.
    #[arg(
        long,
        value_name = "DIRECTORY",
        default_value = "data",
        verbatim_doc_comment
    )]
    config_dir: PathBuf,

    /// Address of the node to connect to for retrieving chain data.
    /// The node should be accessible via the node-2-node protocol, which
    /// means the remote node should be running as a validator and not
    /// as a client node.
    ///
    /// Address is given in the usual `host:port` format, for example: "1.2.3.4:3000".
    #[arg(
        long,
        value_name = "NETWORK_ADDRESS",
        default_value = "127.0.0.1:3001",
        verbatim_doc_comment
    )]
    peer_address: String,
}

pub async fn run(args: Args) -> Result<(), Box<dyn Error>> {
    info!(config=?args.config_dir, ledger_dir=?args.ledger_dir, chain_dir=?args.chain_dir, peer=%args.peer_address, network=%args.network,
          "bootstrapping",
    );

    let network = args.network;
    let era_history = network.into();

    let ledger_dir = args
        .ledger_dir
        .unwrap_or_else(|| default_ledger_dir(args.network).into());
    let chain_dir = args
        .chain_dir
        .unwrap_or_else(|| default_chain_dir(args.network).into());

    let network_dir = args.config_dir.join(&*network.to_string());

    let snapshots_file: PathBuf = network_dir.join("snapshots.json");
    let snapshots_dir = PathBuf::from(snapshots_dir(network));

    download_snapshots(&snapshots_file, &snapshots_dir).await?;

    import_all_from_directory(&ledger_dir, era_history, &snapshots_dir).await?;

    import_nonces_for_network(era_history, &network_dir, &chain_dir).await?;

    import_headers_for_network(network, &args.peer_address, &network_dir, &chain_dir).await?;

    Ok(())
}

async fn import_headers_for_network(
    network: NetworkName,
    peer_address: &str,
    config_dir: &Path,
    chain_dir: &PathBuf,
) -> Result<(), Box<dyn Error>> {
    let headers_file: PathBuf = config_dir.join("headers.json");
    let content = tokio::fs::read_to_string(headers_file).await?;
    let points: Vec<String> = serde_json::from_str(&content)?;
    let mut initial_headers = Vec::new();
    for point_string in points {
        match super::parse_point(&point_string) {
            Ok(point) => initial_headers.push(point),
            Err(e) => tracing::warn!("Ignoring malformed header point '{}': {}", point_string, e),
        }
    }
    for hdr in initial_headers {
        // FIXME: why do we only importa 2 headers for each header listed in the
        // config file? The 2 headers make sense, but why starting from more than
        // one header?
        const NUM_HEADERS_TO_IMPORT: usize = 2;
        import_headers(peer_address, network, chain_dir, hdr, NUM_HEADERS_TO_IMPORT).await?;
    }

    Ok(())
}

async fn import_nonces_for_network(
    era_history: &amaru_kernel::EraHistory,
    config_dir: &Path,
    chain_dir: &PathBuf,
) -> Result<(), Box<dyn Error>> {
    let nonces_file: PathBuf = config_dir.join("nonces.json");
    let content = tokio::fs::read_to_string(nonces_file).await?;
    let initial_nonces: InitialNonces = serde_json::from_str(&content)?;
    import_nonces(era_history, chain_dir, initial_nonces).await?;
    Ok(())
}

/// Configuration for a single ledger state's snapshot to be imported.
#[derive(Debug, Deserialize)]
struct Snapshot {
    /// The snapshot's epoch.
    epoch: u64,

    /// The snapshot's point, in the form `<slot>.<header hash>`.
    ///
    /// TODO: make it a genuine `Point` type.
    point: String,

    /// The URL to retrieve snapshot from.
    url: String,
}

async fn download_snapshots(
    snapshots_file: &PathBuf,
    snapshots_dir: &PathBuf,
) -> Result<(), Box<dyn Error>> {
    // Create the target directory if it doesn't exist
    fs::create_dir_all(snapshots_dir).await?;

    // Read the snapshots JSON file
    let snapshots_content = fs::read_to_string(snapshots_file).await?;
    let snapshots: Vec<Snapshot> = serde_json::from_str(&snapshots_content)?;

    // Create a reqwest client
    let client = reqwest::Client::new();

    // Download each snapshot
    for snapshot in &snapshots {
        info!(epoch=%snapshot.epoch, point=%snapshot.point,
            "Downloading snapshot",
        );

        // Extract filename from the point
        let filename = format!("{}.cbor", snapshot.point);
        let target_path = snapshots_dir.join(&filename);

        // Skip if file already exists
        if target_path.exists() {
            info!("Snapshot {} already exists, skipping", filename);
            continue;
        }

        // Download the file
        let response = client.get(&snapshot.url).send().await?;
        if !response.status().is_success() {
            return Err(format!(
                "Failed to download snapshot from {}: HTTP status {}",
                snapshot.url,
                response.status()
            )
            .into());
        }

        let (tmp_path, file) = uncompress_to_temp_file(&target_path, response).await?;

        file.sync_all().await?;
        tokio::fs::rename(&tmp_path, &target_path).await?;

        info!("Downloaded snapshot to {}", target_path.display());
    }

    info!(
        "All {} snapshots downloaded and decompressed successfully",
        snapshots.len()
    );
    Ok(())
}

async fn uncompress_to_temp_file(
    target_path: &Path,
    response: reqwest::Response,
) -> Result<(PathBuf, File), Box<dyn Error>> {
    let tmp_path = target_path.with_extension("partial");
    let mut file = File::create(&tmp_path).await?;
    let raw_stream_reader = StreamReader::new(response.bytes_stream().map_err(io::Error::other));
    let buffered_reader = BufReader::new(raw_stream_reader);
    let mut decoded_stream = GzipDecoder::new(buffered_reader);
    tokio::io::copy(&mut decoded_stream, &mut file).await?;
    Ok((tmp_path, file))
}
