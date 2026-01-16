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
use amaru::{DEFAULT_NETWORK, DEFAULT_PEER_ADDRESS, bootstrap::BootstrapError, get_bootstrap_file};
use amaru_kernel::{BlockHeader, IsHeader, Point, from_cbor, network::NetworkName, peer::Peer};
use amaru_network::chain_sync_client::ChainSyncClient;
use amaru_progress_bar::{ProgressBar, new_terminal_progress_bar};
use clap::{ArgAction, Parser};
use pallas_network::miniprotocols::chainsync::{HeaderContent, NextResponse};
use std::{
    error::Error,
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::time::timeout;
use tracing::error;

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

    let points: Vec<String> = if args.parent.is_empty() {
        let headers_file_name = "headers.json";
        let content = get_bootstrap_file(network, headers_file_name)?
            .ok_or(BootstrapError::MissingConfigFile(headers_file_name.into()))?;
        serde_json::from_slice(&content)?
    } else {
        args.parent
    };

    fetch_headers_for_network(network, &args.headers_dir, &args.peer_address, points).await?;

    Ok(())
}

async fn fetch_headers_for_network(
    network: NetworkName,
    headers_dir: &Path,
    peer_address: &str,
    points: Vec<String>,
) -> Result<(), Box<dyn Error>> {
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
        fetch_headers(
            peer_address,
            network,
            headers_dir,
            hdr,
            NUM_HEADERS_TO_FETCH,
        )
        .await?;
    }

    Ok(())
}

pub(crate) async fn fetch_headers(
    peer_address: &str,
    network: NetworkName,
    headers_dir: &Path,
    point: Point,
    max: usize,
) -> Result<(), Box<dyn Error>> {
    let peer_client = connect_to_peer(peer_address, &network).await?;
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
            request_next_block(&mut client, headers_dir, &mut count, &mut progress, max).await?
        } else {
            await_for_next_block(&mut client, headers_dir, &mut count, &mut progress, max).await?
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
    headers_dir: &Path,
    count: &mut usize,
    progress: &mut Option<Box<dyn ProgressBar>>,
    max: usize,
) -> Result<What, WorkerError> {
    let next = client.request_next().await.map_err(|err| {
        tracing::warn!(%err, "request next failed");
        WorkerError::Restart
    })?;
    handle_response(headers_dir, next, count, progress, max)
}

async fn await_for_next_block(
    client: &mut ChainSyncClient,
    headers_dir: &Path,
    count: &mut usize,
    progress: &mut Option<Box<dyn ProgressBar>>,
    max: usize,
) -> Result<What, WorkerError> {
    match timeout(Duration::from_secs(1), client.await_next()).await {
        Ok(result) => result
            .map_err(|_| WorkerError::Recv)
            .and_then(|next| handle_response(headers_dir, next, count, progress, max)),
        Err(_) => Err(WorkerError::Retry)?,
    }
}

#[allow(clippy::unwrap_used)]
fn handle_response(
    headers_dir: &Path,
    next: NextResponse<HeaderContent>,
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

            std::fs::create_dir_all(headers_dir)
                .inspect_err(|reason| {
                    error!(
                        dir = %headers_dir.display(),
                        reason = %reason,
                        "Failed to create headers directory"
                    )
                })
                .map_err(|_| WorkerError::Panic)?;

            let filepath = headers_dir.join(&filename);

            let mut file = File::create(&filepath)
                .inspect_err(|reason| {
                    error!(
                        file = %filepath.display(),
                        reason = %reason,
                        "Failed to create file",
                    )
                })
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
