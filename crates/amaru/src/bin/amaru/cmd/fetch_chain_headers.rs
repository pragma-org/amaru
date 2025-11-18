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

use crate::cmd::{WorkerError, connect_to_peer};
use amaru_kernel::{BlockHeader, IsHeader, Point, from_cbor, network::NetworkName, peer::Peer};
use amaru_network::chain_sync_client::ChainSyncClient;
use amaru_progress_bar::{ProgressBar, new_terminal_progress_bar};
use clap::{Parser, arg};
use pallas_network::miniprotocols::chainsync::{HeaderContent, NextResponse};
use std::{
    error::Error,
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::time::timeout;

use tracing::info;

#[derive(Debug, Parser)]
pub struct Args {
    /// Network to fetch chain headers for.
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

    /// Path to directory containing per-network bootstrap configuration files.
    ///
    /// This path will be used as a prefix to resolve per-network configuration files
    /// needed for bootstrapping and to store fetched chain headers. Given a source
    /// directory `data`, and a network name of `preview`, the expected layout for
    /// configuration files would be:
    ///
    /// * `data/preview/snapshots.json`: a list of `Snapshot` values,
    /// * `data/preview/nonces.json`: a list of `InitialNonces` values,
    /// * `data/preview/headers.json`: a list of `Point`s,
    /// * `data/preview/headers`: a directory where the fetched chain headers will be stored.
    #[arg(
        long,
        value_name = "DIR",
        default_value = super::DEFAULT_CONFIG_DIR,
        verbatim_doc_comment,
        env = "AMARU_CONFIG_DIR"
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
        default_value = super::DEFAULT_PEER_ADDRESS,
        verbatim_doc_comment,
        env = "AMARU_PEER_ADDRESS"
    )]
    peer_address: String,
}

pub async fn run(args: Args) -> Result<(), Box<dyn Error>> {
    info!(config=%args.config_dir.to_string_lossy(), peer=%args.peer_address, network=%args.network,
          "Running command fetch-chain-headers",
    );
    let network = args.network;
    let network_dir = args.config_dir.join(&*network.to_string());

    fetch_headers_for_network(network, &args.peer_address, &network_dir).await?;

    Ok(())
}

async fn fetch_headers_for_network(
    network: NetworkName,
    peer_address: &str,
    config_dir: &Path,
) -> Result<(), Box<dyn Error>> {
    let headers_file: PathBuf = config_dir.join("headers.json");
    let content = tokio::fs::read_to_string(headers_file).await?;
    let points: Vec<String> = serde_json::from_str(&content)?;
    let mut initial_headers = Vec::new();
    for point in points {
        let point = Point::try_from(point.as_str())?;
        initial_headers.push(point);
    }
    for hdr in initial_headers {
        // FIXME: why do we only fetch 2 headers for each header listed in the
        // config file? The 2 headers make sense, but why starting from more than
        // one header?
        const NUM_HEADERS_TO_FETCH: usize = 2;
        fetch_headers(peer_address, network, config_dir, hdr, NUM_HEADERS_TO_FETCH).await?;
    }

    Ok(())
}

pub(crate) async fn fetch_headers(
    peer_address: &str,
    network_name: NetworkName,
    config_dir: &Path,
    point: Point,
    max: usize,
) -> Result<(), Box<dyn Error>> {
    let peer_client = connect_to_peer(peer_address, &network_name).await?;
    let mut client = ChainSyncClient::new(
        Peer::new(peer_address),
        peer_client.chainsync,
        vec![point.clone()],
    );
    client.find_intersection().await?;

    let mut count = 0;
    let mut progress: Option<Box<dyn ProgressBar>> = None;

    loop {
        let what = if client.has_agency() {
            request_next_block(&mut client, config_dir, &mut count, &mut progress, max).await?
        } else {
            await_for_next_block(&mut client, config_dir, &mut count, &mut progress, max).await?
        };
        match what {
            Continue => continue,
            Stop => {
                if let Some(progress) = progress {
                    progress.clear()
                }
                break;
            }
        }
    }
    info!(total = count, "header_fetched");
    Ok(())
}
enum What {
    Continue,
    Stop,
}

use What::*;

async fn request_next_block(
    client: &mut ChainSyncClient,
    config_dir: &Path,
    count: &mut usize,
    progress: &mut Option<Box<dyn ProgressBar>>,
    max: usize,
) -> Result<What, WorkerError> {
    let next = client.request_next().await.map_err(|err| {
        tracing::warn!(%err, "request next failed");
        WorkerError::Restart
    })?;
    handle_response(next, config_dir, count, progress, max)
}

async fn await_for_next_block(
    client: &mut ChainSyncClient,
    config_dir: &Path,
    count: &mut usize,
    progress: &mut Option<Box<dyn ProgressBar>>,
    max: usize,
) -> Result<What, WorkerError> {
    match timeout(Duration::from_secs(1), client.await_next()).await {
        Ok(result) => result
            .map_err(|_| WorkerError::Recv)
            .and_then(|next| handle_response(next, config_dir, count, progress, max)),
        Err(_) => Err(WorkerError::Retry)?,
    }
}

#[allow(clippy::unwrap_used)]
fn handle_response(
    next: NextResponse<HeaderContent>,
    config_dir: &Path,
    count: &mut usize,
    progress: &mut Option<Box<dyn ProgressBar>>,
    max: usize,
) -> Result<What, WorkerError> {
    match next {
        NextResponse::RollForward(content, tip) => {
            let header: BlockHeader = from_cbor(&content.cbor).unwrap();
            let hash = header.hash();
            let slot = header.slot();

            let filename = format!("header.{}.{}.cbor", slot, hex::encode(hash));
            let headers_dir = config_dir.join("headers");
            std::fs::create_dir_all(&headers_dir)
                .inspect_err(|reason| tracing::error!(dir = %headers_dir.display(), reason = %reason, "Failed to create headers directory"))
                .map_err(|_| WorkerError::Panic)?;
            let filepath = headers_dir.join(&filename);
            let mut file = File::create(&filepath)
                .inspect_err(|reason| tracing::error!(file = %filepath.display(), reason = %reason, "Failed to create file"))
                .map_err(|_| WorkerError::Panic)?;
            file.write_all(&content.cbor)
                .map_err(|_| WorkerError::Panic)?;

            *count += 1;

            let slot = header.slot();
            let tip_slot = tip.0.slot_or_default();

            if let Some(progress) = progress {
                progress.tick(1)
            }

            if *count >= max || slot == tip_slot {
                Ok(Stop)
            } else {
                Ok(Continue)
            }
        }
        #[allow(clippy::unwrap_used)]
        NextResponse::RollBackward(point, tip) => {
            info!(?point, ?tip, "roll_backward");
            if progress.is_none() {
                *progress = Some(new_terminal_progress_bar(
                    max,
                    " fetching headers (~{eta} left) {bar:70} {pos:>7}/{len:7} ({percent_precise}%)",
                ));
            }
            Ok(Continue)
        }
        NextResponse::Await => Ok(Continue),
    }
}
