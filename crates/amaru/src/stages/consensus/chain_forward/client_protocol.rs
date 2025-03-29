use super::client_state::{find_headers_between, ClientOp, ClientState};
use acto::{ActoCell, ActoInput, ActoRef, ActoRuntime, PanicOrAbort};
use amaru_consensus::consensus::store::ChainStore;
use amaru_kernel::{to_cbor, Header};
use pallas_network::{
    facades::PeerServer,
    miniprotocols::{
        blockfetch,
        chainsync::{self, ClientRequest, HeaderContent, Tip},
        keepalive, txsubmission,
    },
};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("client asked for headers before intersection was found")]
    EarlyRequestNext,
    #[error("no intersection found")]
    NoIntersection,
    #[error("client asked for intersection after it was already found")]
    LateIntersection,
    #[error("client terminated")]
    ClientTerminated,
    #[error("handler failure: {0}")]
    HandlerFailure(PanicOrAbort),
    #[error("chainsync error: {0}")]
    ChainSync(#[from] chainsync::ServerError),
    #[error("block fetch error: {0}")]
    BlockFetch(#[from] blockfetch::ServerError),
    #[error("tx submission error: {0}")]
    TxSubmission(#[from] txsubmission::Error),
    #[error("keep alive error: {0}")]
    KeepAlive(#[from] keepalive::ServerError),
}

pub enum ClientProtocolMsg {
    Op(ClientOp),
}

pub async fn client_protocols(
    mut cell: ActoCell<ClientProtocolMsg, impl ActoRuntime, Result<(), ClientError>>,
    server: PeerServer,
    store: Arc<Mutex<dyn ChainStore<Header>>>,
    tip: Tip,
) -> Result<(), ClientError> {
    let _block_fetch = cell.spawn_supervised("block_fetch", {
        let store = store.clone();
        move |cell| block_fetch(cell, server.blockfetch, store)
    });
    let _tx_submission = cell.spawn_supervised("tx_submission", move |cell| {
        tx_submission(cell, server.txsubmission)
    });
    let _keep_alive =
        cell.spawn_supervised("keep_alive", move |cell| keep_alive(cell, server.keepalive));

    let chain_sync = cell.spawn_supervised("chainsync", move |cell| {
        chain_sync(cell, server.chainsync, tip, store)
    });

    while let ActoInput::Message(msg) = cell.recv().await {
        match msg {
            ClientProtocolMsg::Op(op) => chain_sync.send(ChainSyncMsg::Op(op)),
        };
    }

    Ok(())
}

#[allow(clippy::large_enum_variant)]
pub enum ChainSyncMsg {
    /// An operation coming in from the ledger
    Op(ClientOp),
    /// A request for data from the client
    ReqNext,
}

async fn chain_sync(
    mut cell: ActoCell<ChainSyncMsg, impl ActoRuntime, Result<(), ClientError>>,
    mut server: chainsync::Server<HeaderContent>,
    tip: Tip,
    store: Arc<Mutex<dyn ChainStore<Header>>>,
) -> Result<(), ClientError> {
    // TODO: do we need to handle validation updates already here in case the client is really slow to ask for intersection?
    let Some(ClientRequest::Intersect(req)) = server.recv_while_idle().await? else {
        // need an intersection point to start
        return Err(ClientError::EarlyRequestNext);
    };

    tracing::debug!("finding headers between {:?} and {:?}", tip.0, req);
    let Some((intersection, client_at)) = find_headers_between(&*store.lock().await, &tip.0, &req)
    else {
        tracing::debug!("no intersection found");
        server.send_intersect_not_found(tip).await?;
        return Err(ClientError::NoIntersection);
    };
    tracing::debug!("intersection found: {client_at:?}");
    server
        .send_intersect_found(client_at.0.clone(), tip)
        .await?;

    let mut state = ClientState::new(store, intersection.into(), client_at);

    let parent = cell.me();
    let handler = cell.spawn_supervised("chainsync_handler", move |cell| {
        chain_sync_handler(cell, server, parent)
    });

    let mut waiting = false;
    loop {
        let input = cell.recv().await;
        match input {
            ActoInput::Message(ChainSyncMsg::Op(op)) => {
                state.add_op(op);
                if waiting {
                    if let Some((op, tip)) = state.next_op().await {
                        waiting = false;
                        handler.send(Some((op, tip)));
                    }
                }
            }
            ActoInput::Message(ChainSyncMsg::ReqNext) => {
                if let Some((op, tip)) = state.next_op().await {
                    handler.send(Some((op, tip)));
                } else {
                    handler.send(None);
                    waiting = true;
                }
            }
            ActoInput::NoMoreSenders => return Ok(()),
            ActoInput::Supervision { result, .. } => {
                return result.map_err(ClientError::HandlerFailure).and_then(|x| x);
            }
        }
    }
}

/// This actor handles the ReqNext part of the chainsync protocol.
/// It will ask the parent for the next operation whenever needed.
/// The parent may respond with None to indicate the need to await, then it will eventually send the next operation.
async fn chain_sync_handler(
    mut cell: ActoCell<Option<(ClientOp, Tip)>, impl ActoRuntime>,
    mut server: chainsync::Server<HeaderContent>,
    parent: ActoRef<ChainSyncMsg>,
) -> Result<(), ClientError> {
    loop {
        let Some(req) = server.recv_while_idle().await? else {
            return Err(ClientError::ClientTerminated);
        };
        if !matches!(req, ClientRequest::RequestNext) {
            return Err(ClientError::LateIntersection);
        };
        parent.send(ChainSyncMsg::ReqNext);

        if let ActoInput::Message(op) = cell.recv().await {
            match op {
                Some((ClientOp::Forward(header), tip)) => {
                    server
                        .send_roll_forward(to_header_content(header), tip)
                        .await?;
                }
                Some((ClientOp::Backward(point), tip)) => {
                    server.send_roll_backward(point, tip).await?;
                }
                None => {
                    server.send_await_reply().await?;
                    let ActoInput::Message(Some((op, tip))) = cell.recv().await else {
                        return Ok(());
                    };
                    match op {
                        ClientOp::Forward(header) => {
                            server
                                .send_roll_forward(to_header_content(header), tip)
                                .await?;
                        }
                        ClientOp::Backward(point) => {
                            server.send_roll_backward(point, tip).await?;
                        }
                    }
                }
            }
        } else {
            // parent terminated
            return Ok(());
        }
    }
}

fn to_header_content(header: Header) -> HeaderContent {
    HeaderContent {
        variant: 1,
        byron_prefix: None,
        cbor: to_cbor(&header),
    }
}

enum BlockFetchMsg {}

async fn block_fetch(
    _cell: ActoCell<BlockFetchMsg, impl ActoRuntime>,
    mut server: blockfetch::Server,
    _store: Arc<Mutex<dyn ChainStore<Header>>>, // TODO: need a block store here
) -> Result<(), ClientError> {
    while let Some(req) = server.recv_while_idle().await? {
        tracing::info!("block fetch request: {:?}", req);
        // TODO: Implement block fetch
        server.send_no_blocks().await?;
    }

    Ok(())
}

enum TxSubmissionMsg {}

async fn tx_submission(
    _cell: ActoCell<TxSubmissionMsg, impl ActoRuntime>,
    mut server: txsubmission::Server,
) -> Result<(), ClientError> {
    server.wait_for_init().await?;

    // TODO: Implement tx submission

    Ok(())
}

enum KeepAliveMsg {}

async fn keep_alive(
    _cell: ActoCell<KeepAliveMsg, impl ActoRuntime>,
    mut server: keepalive::Server,
) -> Result<(), ClientError> {
    loop {
        server.keepalive_roundtrip().await?;
    }
}
