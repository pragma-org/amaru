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

use amaru_consensus::{IsHeader, consensus::store::ChainStore};
use amaru_kernel::{Header, Point, default_chain_dir, from_cbor, network::NetworkName};
use amaru_network::{
    chain_sync_client::{PullResult, new_with_peer},
    connect_to_peer,
};
use amaru_progress_bar::{ProgressBar, new_terminal_progress_bar};
use amaru_stores::rocksdb::consensus::RocksDBStore;
use clap::Parser;
use gasket::framework::*;
use pallas_network::miniprotocols::chainsync::HeaderContent;
use std::{error::Error, path::PathBuf};
use tracing::info;

#[derive(Debug, Parser)]
pub struct Args {
    /// Address of the node to connect to for retrieving chain data.
    /// The node should be accessible via the node-2-node protocol, which
    /// means the remote node should be running as a validator and not
    /// as a client node.
    ///
    /// Addressis given in the usual `host:port` format, for example: "1.2.3.4:3000".
    #[arg(long, value_name = "NETWORK_ADDRESS", verbatim_doc_comment)]
    peer_address: String,

    /// Network to use for the connection.
    ///
    /// Should be one of 'mainnet', 'preprod', 'preview' or 'testnet:<magic>' where
    /// `magic` is a 32-bits unsigned value denoting a particular testnet.
    #[arg(
        long,
        value_name = "NETWORK",
        default_value_t = NetworkName::Preprod,
    )]
    network: NetworkName,

    /// Path of the consensus on-disk storage.
    ///
    /// This is the directory where data will be stored. The directory and any intermediate
    /// paths will be created if they do not exist.
    #[arg(long, value_name = "DIR", verbatim_doc_comment)]
    chain_dir: Option<PathBuf>,

    /// Starting point of import.
    ///
    /// This is the "intersection" point which will be given to the peer as a starting point
    /// to import the chain database.
    #[arg(long, value_name = "POINT", verbatim_doc_comment, value_parser = |s: &str| Point::try_from(s))]
    starting_point: Point,

    /// Number of headers to import.
    /// Maximum number of headers to import from the `peer`.
    /// By default, it will retrieve all headers until it reaches the tip of the peer's chain.
    #[arg(long, value_name = "UINT", verbatim_doc_comment, default_value_t = usize::MAX)]
    count: usize,
}

pub async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let chain_dir = args
        .chain_dir
        .unwrap_or_else(|| default_chain_dir(args.network).into());
    import_headers(
        &args.peer_address,
        args.network,
        &chain_dir,
        args.starting_point,
        args.count,
    )
    .await
}

pub(crate) async fn import_headers(
    peer_address: &str,
    network_name: NetworkName,
    chain_db_dir: &PathBuf,
    point: Point,
    max: usize,
) -> Result<(), Box<dyn Error>> {
    let era_history = network_name.into();
    let mut db = RocksDBStore::new(chain_db_dir, era_history)?;

    let mut count = 0;

    let mut progress: Option<Box<dyn ProgressBar>> = None;

    let peer = connect_to_peer(peer_address, &network_name).await?;

    let mut chain_sync = new_with_peer(peer, &[point]);

    chain_sync.find_intersection().await?;

    while count < max {
        match chain_sync.pull_batch().await? {
            PullResult::ForwardBatch(header_contents) => {
                handle_batch(&header_contents, &mut db, &mut progress)?;
                count += header_contents.len()
            }
            PullResult::RollBack(point, tip) => {
                info!(?point, ?tip, "roll_backward");
                if progress.is_none() {
                    progress = Some(new_terminal_progress_bar(
                        max,
                        " importing headers (~{eta} left) {bar:70} {pos:>7}/{len:7} ({percent_precise}%)",
                    ));
                }
            }
            PullResult::Nothing => break,
        }
    }

    info!(total = count, "header_imported");
    Ok(())
}

#[allow(clippy::unwrap_used)]
fn handle_batch(
    next: &Vec<HeaderContent>,
    db: &mut RocksDBStore,
    progress: &mut Option<Box<dyn ProgressBar>>,
) -> Result<(), WorkerError> {
    for content in next {
        let header: Header = from_cbor(&content.cbor).unwrap();
        let hash = header.hash();

        db.store_header(&hash, &header)
            .map_err(|_| WorkerError::Panic)?;

        if let Some(progress) = progress {
            progress.tick(1)
        }
    }

    Ok(())
}
