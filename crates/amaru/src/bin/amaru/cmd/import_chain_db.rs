use crate::config::NetworkName;
use amaru::sync;
use amaru_consensus::{
    consensus::{
        header::{ConwayHeader, Header},
        store::{ChainStore, rocksdb::RocksDBStore},
    },
    peer::{Peer, PeerSession},
};
use clap::{Parser, builder::TypedValueParser as _};
use gasket::framework::*;
use indicatif::{ProgressBar, ProgressStyle};
use pallas_network::{
    facades::PeerClient,
    miniprotocols::chainsync::{self, HeaderContent, NextResponse},
};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::{sync::Mutex, time::timeout};
use tracing::info;

use super::parse_point;

#[derive(Debug, Parser)]
pub struct Args {
    /// Address of the node to connect to for retrieving chain data.
    /// The node should be accessible via the node-2-node protocol, which
    /// means the remote node should be running as a validator and not
    /// as a client node.
    ///
    /// Addressis given in the usual `host:port` format, for example: "1.2.3.4:3000".
    #[arg(long, verbatim_doc_comment)]
    peer_address: String,

    /// Network to use for the connection.
    #[arg(
        long,
        default_value_t = NetworkName::Preprod,
        value_parser = clap::builder::PossibleValuesParser::new(NetworkName::possible_values())
            .map(|s| s.parse::<NetworkName>().unwrap()),
    )]
    network: NetworkName,

    /// Path of the on-disk storage.
    ///
    /// This is the directory where data will be stored. The directory and any intermediate
    /// paths will be created if they do not exist.
    #[arg(long, verbatim_doc_comment, default_value = super::DEFAULT_CHAIN_DATABASE_PATH)]
    chain_dir: PathBuf,

    /// Starting point of import.
    ///
    /// This is the "intersection" point which will be given to the peer as a starting point
    /// to import the chain database.
    #[arg(long, verbatim_doc_comment)]
    starting_point: String,

    /// Number of headers to import.
    /// Maximum number of headers to import from the `peer`.
    /// By default, it will retrieve all headers until it reaches the tip of the peer's chain.
    #[arg(long, verbatim_doc_comment, default_value_t = usize::MAX)]
    count: usize,
}

#[derive(Debug, thiserror::Error)]
enum Error<'a> {
    #[error("malformed point: {}", .0)]
    MalformedPoint(&'a str),
}

enum What {
    Continue,
    Stop,
}

use What::*;

pub async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let mut db = RocksDBStore::new(args.chain_dir)?;

    let peer_client = Arc::new(Mutex::new(
        PeerClient::connect(
            args.peer_address.clone(),
            args.network.to_network_magic() as u64,
        )
        .await?,
    ));

    let peer_session = PeerSession {
        peer: Peer::new(&args.peer_address),
        peer_client,
    };

    let point = parse_point(args.starting_point.as_str(), Error::MalformedPoint)?;

    let mut pull = sync::pull::Stage::new(peer_session.clone(), vec![point.clone()]);

    pull.find_intersection().await?;

    let mut peer_client = pull.peer_session.lock().await;
    let mut count = 0;
    let max = args.count;
    let start = point.slot_or_default();

    let client = (*peer_client).chainsync();

    let mut progress: Option<ProgressBar> = None;

    // TODO: implement a proper pipelined client because this one is super slow
    // Pipelining in Haskell is single threaded which implies the code handles
    // scheduling between sending burst of MsgRequest and collecting responses.
    // Here we can do better thanks to gasket's workers: just spawn 2 workers,
    // one for sending requests and the other for handling responses, along
    // with a shared counter.
    // Pipelining stops when we reach the tip of the peer's chain.
    loop {
        let what = if client.has_agency() {
            request_next_block(client, &mut db, &mut count, &mut progress, max, start).await?
        } else {
            await_for_next_block(client, &mut db, &mut count, &mut progress, max, start).await?
        };
        match what {
            Continue => continue,
            Stop => {
                if let Some(progress) = progress {
                    progress.finish_and_clear()
                }
                break;
            }
        }
    }
    info!(total = count, "header_imported");
    Ok(())
}

async fn request_next_block(
    client: &mut chainsync::Client<HeaderContent>,
    db: &mut RocksDBStore,
    count: &mut usize,
    progress: &mut Option<ProgressBar>,
    max: usize,
    start: u64,
) -> Result<What, WorkerError> {
    let next = client.request_next().await.or_restart()?;
    handle_response(next, db, count, progress, max, start)
}

async fn await_for_next_block(
    client: &mut chainsync::Client<HeaderContent>,
    db: &mut RocksDBStore,
    count: &mut usize,
    progress: &mut Option<ProgressBar>,
    max: usize,
    start: u64,
) -> Result<What, WorkerError> {
    match timeout(Duration::from_secs(1), client.recv_while_must_reply()).await {
        Ok(result) => result
            .map_err(|_| WorkerError::Recv)
            .and_then(|next| handle_response(next, db, count, progress, max, start)),
        Err(_) => Err(WorkerError::Retry)?,
    }
}

#[allow(clippy::unwrap_used)]
fn handle_response(
    next: NextResponse<HeaderContent>,
    db: &mut RocksDBStore,
    count: &mut usize,
    progress: &mut Option<ProgressBar>,
    max: usize,
    start: u64,
) -> Result<What, WorkerError> {
    match next {
        NextResponse::RollForward(content, tip) => {
            let header = ConwayHeader::from_cbor(&content.cbor).unwrap();
            let hash = header.hash();

            db.store_header(&hash, &header)
                .map_err(|_| WorkerError::Panic)?;

            *count += 1;

            let slot = header.slot();
            let tip_slot = tip.0.slot_or_default();

            if let Some(progress) = progress {
                progress.set_position(slot - start);
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
            let tip_slot = tip.0.slot_or_default();
            if progress.is_none() {
                *progress = Some(
                    ProgressBar::new(tip_slot - start).with_style(
                        ProgressStyle::with_template(
                            " importing headers (~{eta} left) {bar:70} {pos:>7}/{len:7} ({percent_precise}%)",
                        )
                        .unwrap(),
                    ),
                );
                if let Some(progress) = progress {
                    progress.tick();
                }
            }
            Ok(Continue)
        }
        NextResponse::Await => Ok(Continue),
    }
}
