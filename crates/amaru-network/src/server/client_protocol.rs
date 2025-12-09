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

use crate::point::from_network_point;

use super::chain_follower::ChainFollower;
use crate::server::as_tip::AsTip;
use crate::tx_submission::conversions::{new_era_tx_id, tx_from_era_tx_body, tx_id_and_size};
use acto::{ActoCell, ActoInput, ActoRef, ActoRuntime};
use amaru_kernel::peer::Peer;
use amaru_kernel::{BlockHeader, Hash, HeaderHash, IsHeader, Tx, to_cbor};
use amaru_ouroboros_traits::{ChainStore, TxClientReply, TxId, TxServerRequest};
use pallas_network::miniprotocols::txsubmission::Reply;
use pallas_network::{
    facades::PeerServer,
    miniprotocols::{
        Point,
        blockfetch::{self, BlockRequest},
        chainsync::{self, ClientRequest, HeaderContent, Tip},
        keepalive, txsubmission,
    },
};
use std::sync::Arc;
use tokio::select;
use tokio::sync::mpsc::Sender;
use tracing::Span;

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChainSyncOp {
    /// the tip to go back to
    Backward(Tip),
    /// the header to go forward to and the tip we will be at after sending this header
    Forward(BlockHeader),
}

impl ChainSyncOp {
    pub fn tip(&self) -> Tip {
        match self {
            ChainSyncOp::Backward(tip) => tip.clone(),
            ChainSyncOp::Forward(header) => header.as_tip(),
        }
    }
}

pub enum ClientProtocolMsg {
    ChainSync(ChainSyncOp),
    TxSubmission(TxServerRequest),
}

pub struct PrettyPoint<'a>(pub &'a Point);

impl std::fmt::Debug for PrettyPoint<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "({}, {})",
            self.0.slot_or_default(),
            hex::encode(hash_point(self.0))
        )
    }
}

pub(crate) fn hash_point(point: &Point) -> HeaderHash {
    match point {
        Point::Origin => Hash::from([0; 32]),
        Point::Specific(_slot, hash) => Hash::from(hash.as_slice()),
    }
}

pub enum ClientMsg {
    /// A new peer has connected to us.
    ///
    /// Our tip is included to get the connection handlers started correctly.
    Peer(PeerServer, Tip),
    /// An operation to be executed on all clients.
    ChainSync(ChainSyncOp),
    /// An tx submission operation to be executed on a specific client.
    TxSubmission(TxServerRequest),
}

pub async fn client_protocols(
    mut cell: ActoCell<ClientProtocolMsg, impl ActoRuntime, anyhow::Result<()>>,
    server: PeerServer,
    peer: Peer,
    store: Arc<dyn ChainStore<BlockHeader>>,
    our_tip: Tip,
    tx_client_reply_sender: Sender<TxClientReply>,
) -> anyhow::Result<()> {
    let _block_fetch = cell.spawn_supervised("block_fetch", {
        let store = store.clone();
        move |cell| block_fetch(cell, server.blockfetch, store)
    });
    let tx_submission = cell.spawn_supervised("tx_submission", move |cell| {
        tx_submission(cell, server.txsubmission, peer, tx_client_reply_sender)
    });
    let _keep_alive =
        cell.spawn_supervised("keep_alive", move |cell| keep_alive(cell, server.keepalive));

    let chain_sync = cell.spawn_supervised("chain_sync", move |cell| {
        chain_sync(cell, server.chainsync, our_tip, store)
    });

    while let ActoInput::Message(msg) = cell.recv().await {
        match msg {
            ClientProtocolMsg::ChainSync(op) => chain_sync.send(ChainSyncMsg::Op(op)),
            ClientProtocolMsg::TxSubmission(op) => tx_submission.send(TxSubmissionMsg::Op(op)),
        };
    }

    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChainSyncMsg {
    /// An operation coming in from the ledger
    Op(ChainSyncOp),
    /// A request for data from the client
    ReqNext,
}

async fn chain_sync(
    mut cell: ActoCell<ChainSyncMsg, impl ActoRuntime, anyhow::Result<()>>,
    mut server: chainsync::Server<HeaderContent>,
    our_tip: Tip,
    store: Arc<dyn ChainStore<BlockHeader>>,
) -> anyhow::Result<()> {
    // TODO: do we need to handle validation updates already here in case the client is really slow to ask for intersection?
    let Some(ClientRequest::Intersect(requested_points)) = server.recv_while_idle().await? else {
        // need an intersection point to start
        return Err(ClientError::EarlyRequestNext.into());
    };

    tracing::debug!(
        points = ?requested_points,
        tip = ?our_tip,
        "request interception",
    );

    let Some(mut chain_follower) = ChainFollower::new(store.clone(), &our_tip.0, &requested_points)
    else {
        tracing::debug!("no intersection found");
        server.send_intersect_not_found(our_tip).await?;
        return Err(ClientError::NoIntersection.into());
    };

    let intersection = chain_follower.intersection_found();

    tracing::debug!(intersection = ?intersection, "intersection found");

    server
        .send_intersect_found(intersection, our_tip.clone())
        .await?;

    let parent = cell.me();
    let chainsync_handler = cell.spawn_supervised("chainsync_handler", move |cell| {
        chain_sync_handler(cell, server, parent)
    });

    let mut our_tip = our_tip;
    let mut waiting = false;
    loop {
        let input = cell.recv().await;
        match input {
            ActoInput::Message(ChainSyncMsg::Op(op)) => {
                tracing::trace!(operation = ?op, "forward change");
                our_tip = op.tip();
                chain_follower.add_op(op);
                if waiting && let Some(op) = chain_follower.next_op(store.clone()) {
                    tracing::trace!(operation = ?op, "reply await");
                    waiting = false;
                    chainsync_handler.send(Some((op, our_tip.clone())));
                }
            }
            ActoInput::Message(ChainSyncMsg::ReqNext) => {
                tracing::trace!("client request next");
                if let Some(op) = chain_follower.next_op(store.clone()) {
                    tracing::trace!(operation = ?op, "forward next operation");
                    chainsync_handler.send(Some((op, our_tip.clone())));
                } else {
                    tracing::trace!("sending await reply");
                    chainsync_handler.send(None);
                    waiting = true;
                }
            }
            ActoInput::NoMoreSenders => {
                tracing::trace!("no more senders");
                return Ok(());
            }
            ActoInput::Supervision { result, .. } => {
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
    mut cell: ActoCell<Option<(ChainSyncOp, Tip)>, impl ActoRuntime>,
    mut server: chainsync::Server<HeaderContent>,
    parent: ActoRef<ChainSyncMsg>,
) -> anyhow::Result<()> {
    loop {
        let Some(req) = server.recv_while_idle().await? else {
            tracing::debug!("client terminated");
            return Err(ClientError::ClientTerminated.into());
        };
        // FIXME: a client should always be able to request an intersection
        if !matches!(req, ClientRequest::RequestNext) {
            tracing::debug!("late intersection");
            return Err(ClientError::LateIntersection.into());
        };

        parent.send(ChainSyncMsg::ReqNext);

        if let ActoInput::Message(op) = cell.recv().await {
            match op {
                Some((ChainSyncOp::Forward(header), tip)) => {
                    tracing::trace!(?tip, "roll forward");
                    server
                        .send_roll_forward(to_header_content(header), tip)
                        .await?;
                }
                Some((ChainSyncOp::Backward(point), tip)) => {
                    tracing::trace!(?point, "roll backward");
                    server.send_roll_backward(point.0, tip).await?;
                }
                None => {
                    tracing::trace!("await reply");
                    server.send_await_reply().await?;
                    let ActoInput::Message(Some((op, tip))) = cell.recv().await else {
                        return Ok(());
                    };
                    match op {
                        ChainSyncOp::Forward(header) => {
                            server
                                .send_roll_forward(to_header_content(header), tip)
                                .await?;
                        }
                        ChainSyncOp::Backward(point) => {
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

pub(super) fn to_header_content<H: IsHeader>(header: H) -> HeaderContent {
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
    store: Arc<dyn ChainStore<BlockHeader>>, // TODO: need a block store here
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

        let block = store.load_block(&Hash::from(&from_network_point(&lb_point)))?;
        server.send_start_batch().await?;
        server.send_block(block.to_vec()).await?;
        server.send_batch_done().await?;
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TxSubmissionMsg {
    Op(TxServerRequest),
}

async fn tx_submission(
    mut cell: ActoCell<TxSubmissionMsg, impl ActoRuntime>,
    mut server: txsubmission::Server,
    peer: Peer,
    tx_client_reply_sender: Sender<TxClientReply>,
) -> anyhow::Result<()> {
    server.wait_for_init().await?;
    loop {
        select! {
            reply = server.receive_next_reply() => {
                let tx_client_reply = match reply? {
                    Reply::TxIds(tx_ids) => {
                        let tx_ids: Vec<(TxId, u32)> = tx_ids.into_iter().map(tx_id_and_size).collect();
                        TxClientReply::TxIds { peer: peer.clone(), tx_ids, span: Span::current() }
                    },
                    Reply::Txs(tx_bodies) => {
                        let mut txs: Vec<Tx> = vec![];
                        for tx_body in tx_bodies {
                            let tx = tx_from_era_tx_body(&tx_body)?;
                            txs.push(tx);
                        }
                        TxClientReply::Txs { peer: peer.clone(), txs, span: Span::current() }
                    },
                    Reply::Done => {
                        break;
                    },
                };
                tx_client_reply_sender.send(tx_client_reply).await.ok();
            },
            message = cell.recv() => {
                match message {
                    ActoInput::Message(TxSubmissionMsg::Op(TxServerRequest::TxIds { ack, req, .. })) => {
                        server
                            .acknowledge_and_request_tx_ids(true, ack, req)
                            .await?
                    },
                    ActoInput::Message(TxSubmissionMsg::Op(TxServerRequest::TxIdsNonBlocking { ack, req, .. })) => {
                        server
                            .acknowledge_and_request_tx_ids(false, ack, req)
                            .await?
                    },
                    ActoInput::Message(TxSubmissionMsg::Op(TxServerRequest::Txs { tx_ids, .. })) => {
                        server
                            .request_txs(tx_ids.into_iter().map(new_era_tx_id).collect())
                            .await?
                    },
                    ActoInput::NoMoreSenders | ActoInput::Supervision { .. }  => {
                        tracing::debug!("parent terminated");
                        break;

                   }
                }
            }
        }
    }
    Ok(())
}

enum KeepAliveMsg {}

async fn keep_alive(
    _cell: ActoCell<KeepAliveMsg, impl ActoRuntime>,
    mut server: keepalive::Server,
) -> anyhow::Result<()> {
    loop {
        server.keepalive_roundtrip().await?
    }
}
