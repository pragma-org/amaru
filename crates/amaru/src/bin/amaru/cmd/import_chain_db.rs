use amaru::{
    consensus::{
        header::{ConwayHeader, Header},
        store::{rocksdb::RocksDBStore, ChainStore},
        Peer, PeerSession,
    },
    sync::pull::Stage,
};
use clap::Parser;
use gasket::framework::*;
use miette::{Diagnostic, IntoDiagnostic};
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
    peer: String,

    /// Network magic to use for the connection.
    ///
    /// Should be 1 for preprod, 2 for preview.
    #[arg(long, verbatim_doc_comment, default_value = "1")]
    network_magic: u32,

    /// Path of the on-disk storage.
    ///
    /// This is the directory where data will be stored. The directory and any intermediate
    /// paths will be created if they do not exist.
    #[arg(long, verbatim_doc_comment, default_value = super::DEFAULT_CHAIN_DATABASE_PATH)]
    chain_database_dir: PathBuf,

    /// Starting point of import.
    ///
    /// This is the "intersection" point which will be given to the peer as a starting point
    /// to import the chain database.
    #[arg(long, verbatim_doc_comment)]
    starting_point: String,

    /// Number of headers to import.
    /// Maximum number of headers to import from the `peer`.
    /// If 0, then all available headers are retrieved, eg. until we reach the tip.
    #[arg(long, verbatim_doc_comment, default_value = "0")]
    count: u64,
}

#[derive(Debug, thiserror::Error, Diagnostic)]
enum Error<'a> {
    #[error("malformed point: {}", .0)]
    MalformedPoint(&'a str),
}

pub async fn run(args: Args) -> miette::Result<()> {
    let mut db = RocksDBStore::new(args.chain_database_dir)?;

    let peer_client = Arc::new(Mutex::new(
        PeerClient::connect(args.peer.clone(), args.network_magic as u64)
            .await
            .into_diagnostic()?,
    ));

    let peer_session = PeerSession {
        peer: Peer::new(&args.peer),
        peer_client,
    };

    let point = parse_point(args.starting_point.as_str(), Error::MalformedPoint).unwrap();

    let mut pull = Stage::new(peer_session.clone(), vec![point]);

    pull.find_intersection().await.into_diagnostic()?;

    let mut peer_client = pull.peer_session.lock().await;
    let client = (*peer_client).chainsync();

    if client.has_agency() {
        request_next_block(client, &mut db)
            .await
            .into_diagnostic()?;
    } else {
        await_for_next_block(client, &mut db)
            .await
            .into_diagnostic()?;
    }

    Ok(())
}

async fn request_next_block(
    client: &mut chainsync::Client<HeaderContent>,
    db: &mut RocksDBStore,
) -> Result<(), WorkerError> {
    let next = client.request_next().await.or_restart()?;
    handle_response(next, db)?;

    Ok(())
}

async fn await_for_next_block(
    client: &mut chainsync::Client<HeaderContent>,
    db: &mut RocksDBStore,
) -> Result<(), WorkerError> {
    match timeout(Duration::from_secs(1), client.recv_while_must_reply()).await {
        Ok(result) => result
            .map_err(|_| WorkerError::Recv)
            .and_then(|next| handle_response(next, db)),
        Err(_) => Err(WorkerError::Retry)?,
    }
}

fn handle_response(
    next: NextResponse<HeaderContent>,
    db: &mut RocksDBStore,
) -> Result<(), WorkerError> {
    match next {
        NextResponse::RollForward(content, _tip) => {
            let header = ConwayHeader::from_cbor(&content.cbor).unwrap();
            let hash = header.hash();

            db.put(&hash, &header).map_err(|_| WorkerError::Panic)?
        }
        NextResponse::RollBackward(point, tip) => {
            info!("rollback received {:?}/{:?}", point, tip)
        }
        NextResponse::Await => {}
    };
    Ok(())
}
