use crate::point::from_network_point;

use super::{
    client_state::{find_headers_between, ClientState},
    ClientOp,
};
use acto::{ActoCell, ActoInput, ActoRef, ActoRuntime};
use amaru_consensus::consensus::store::ChainStore;
use amaru_kernel::{to_cbor, Hash, Header};
use pallas_network::{
    facades::PeerServer,
    miniprotocols::{
        blockfetch::{self, BlockRequest},
        chainsync::{self, ClientRequest, HeaderContent, Tip},
        keepalive, txsubmission, Point,
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
    HandlerFailure(String),
    #[error("does not know how to serve range of points: {0:?} {1:?}")]
    CannotServeRange(Point, Point),
}

pub enum ClientProtocolMsg {
    Op(ClientOp),
}

pub async fn client_protocols(
    mut cell: ActoCell<ClientProtocolMsg, impl ActoRuntime, anyhow::Result<()>>,
    server: PeerServer,
    store: Arc<Mutex<dyn ChainStore<Header>>>,
    our_tip: Tip,
) -> anyhow::Result<()> {
    let _block_fetch = cell.spawn_supervised("block_fetch", {
        let store = store.clone();
        move |cell| block_fetch(cell, server.blockfetch, store)
    });
    let _tx_submission = cell.spawn_supervised("tx_submission", move |cell| {
        tx_submission(cell, server.txsubmission)
    });
    let _keep_alive =
        cell.spawn_supervised("keep_alive", move |cell| keep_alive(cell, server.keepalive));

    let chain_sync = cell.spawn_supervised("chain_sync", move |cell| {
        chain_sync(cell, server.chainsync, our_tip, store)
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
    mut cell: ActoCell<ChainSyncMsg, impl ActoRuntime, anyhow::Result<()>>,
    mut server: chainsync::Server<HeaderContent>,
    our_tip: Tip,
    store: Arc<Mutex<dyn ChainStore<Header>>>,
) -> anyhow::Result<()> {
    // TODO: do we need to handle validation updates already here in case the client is really slow to ask for intersection?
    let Some(ClientRequest::Intersect(req)) = server.recv_while_idle().await? else {
        // need an intersection point to start
        return Err(ClientError::EarlyRequestNext.into());
    };

    tracing::debug!("finding headers between {:?} and {:?}", our_tip.0, req);
    let Some((catch_up, client_at)) = find_headers_between(&*store.lock().await, &our_tip.0, &req)
    else {
        tracing::debug!("no intersection found");
        server.send_intersect_not_found(our_tip).await?;
        return Err(ClientError::NoIntersection.into());
    };
    tracing::debug!("intersection found: {client_at:?}");
    server
        .send_intersect_found(client_at.0.clone(), our_tip.clone())
        .await?;

    let parent = cell.me();
    let handler = cell.spawn_supervised("chainsync_handler", move |cell| {
        chain_sync_handler(cell, server, parent)
    });

    let mut state = ClientState::new(catch_up.into());
    let mut our_tip = our_tip;
    let mut waiting = false;
    loop {
        let input = cell.recv().await;
        match input {
            ActoInput::Message(ChainSyncMsg::Op(op)) => {
                tracing::debug!("got op {op:?}");
                our_tip = op.tip();
                state.add_op(op);
                if waiting {
                    if let Some(op) = state.next_op() {
                        tracing::debug!("sending op {op:?} to waiting handler");
                        waiting = false;
                        handler.send(Some((op, our_tip.clone())));
                    }
                }
            }
            ActoInput::Message(ChainSyncMsg::ReqNext) => {
                tracing::debug!("got req next");
                if let Some(op) = state.next_op() {
                    tracing::debug!("sending op {op:?} to handler");
                    handler.send(Some((op, our_tip.clone())));
                } else {
                    tracing::debug!("sending await reply");
                    handler.send(None);
                    waiting = true;
                }
            }
            ActoInput::NoMoreSenders => {
                tracing::debug!("no more senders");
                return Ok(());
            }
            ActoInput::Supervision { result, .. } => {
                tracing::debug!("supervision result: {result:?}");
                return result
                    .map_err(|e| anyhow::Error::from(ClientError::HandlerFailure(e.to_string())))
                    .and_then(|x| x);
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
) -> anyhow::Result<()> {
    loop {
        let Some(req) = server.recv_while_idle().await? else {
            tracing::debug!("client terminated");
            return Err(ClientError::ClientTerminated.into());
        };
        if !matches!(req, ClientRequest::RequestNext) {
            tracing::debug!("late intersection");
            return Err(ClientError::LateIntersection.into());
        };
        tracing::debug!("got req next");
        parent.send(ChainSyncMsg::ReqNext);

        if let ActoInput::Message(op) = cell.recv().await {
            match op {
                Some((ClientOp::Forward(header, _), tip)) => {
                    tracing::debug!("sending roll forward");
                    server
                        .send_roll_forward(to_header_content(header), tip)
                        .await?;
                }
                Some((ClientOp::Backward(point), tip)) => {
                    tracing::debug!("sending roll backward");
                    server.send_roll_backward(point.0, tip).await?;
                }
                None => {
                    tracing::debug!("sending await reply");
                    server.send_await_reply().await?;
                    let ActoInput::Message(Some((op, tip))) = cell.recv().await else {
                        return Ok(());
                    };
                    match op {
                        ClientOp::Forward(header, _) => {
                            server
                                .send_roll_forward(to_header_content(header), tip)
                                .await?;
                        }
                        ClientOp::Backward(point) => {
                            server.send_roll_backward(point.0, tip).await?;
                        }
                    }
                }
            }
        } else {
            tracing::debug!("parent terminated");
            // parent terminated
            return Ok(());
        }
    }
}

pub(super) fn to_header_content(header: Header) -> HeaderContent {
    HeaderContent {
        variant: 6,
        byron_prefix: None,
        cbor: to_cbor(&header),
    }
}

enum BlockFetchMsg {}

async fn block_fetch(
    _cell: ActoCell<BlockFetchMsg, impl ActoRuntime>,
    mut server: blockfetch::Server,
    store: Arc<Mutex<dyn ChainStore<Header>>>, // TODO: need a block store here
) -> anyhow::Result<()> {
    loop {
        let Some(req) = server.recv_while_idle().await? else {
            return Err(ClientError::ClientTerminated.into());
        };
        let BlockRequest((lb_point, ub_point)) = req;

        // FIXME: we should be able to iterate between the 2 points to serve a
        // range properly, which implies knowing how to iterate over the chain
        // db between 2 points.
        if lb_point != ub_point {
            return Err(ClientError::CannotServeRange(lb_point, ub_point).into());
        }

        let db = store.lock().await;
        let block = db.load_block(&Hash::from(&from_network_point(&lb_point)))?;
        server.send_start_batch().await?;
        server.send_block(block).await?;
        server.send_batch_done().await?;
    }
}

enum TxSubmissionMsg {}

async fn tx_submission(
    _cell: ActoCell<TxSubmissionMsg, impl ActoRuntime>,
    mut server: txsubmission::Server,
) -> anyhow::Result<()> {
    server.wait_for_init().await?;

    // TODO: Implement tx submission

    Ok(())
}

enum KeepAliveMsg {}

async fn keep_alive(
    _cell: ActoCell<KeepAliveMsg, impl ActoRuntime>,
    mut server: keepalive::Server,
) -> anyhow::Result<()> {
    loop {
        server.keepalive_roundtrip().await?;
    }
}
