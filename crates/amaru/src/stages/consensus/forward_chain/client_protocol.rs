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

use super::client_state::{ClientState, find_headers_between};
use crate::stages::AsTip;
use acto::{ActoCell, ActoInput, ActoRef, ActoRuntime};
use amaru_consensus::ChainStore;
use amaru_kernel::{Hash, HeaderHash, IsHeader, to_cbor};
use amaru_network::point::to_network_point;
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

#[derive(Clone)]
pub enum ClientOp<H> {
    /// the tip to go back to
    Backward(Tip),
    /// the header to go forward to and the tip we will be at after sending this header
    Forward(H),
}

impl<H: PartialEq> PartialEq for ClientOp<H> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Backward(l0), Self::Backward(r0)) => (&l0.0, l0.1) == (&r0.0, r0.1),
            (Self::Forward(l0), Self::Forward(r0)) => l0 == r0,
            _ => false,
        }
    }
}

impl<H: Eq> Eq for ClientOp<H> {}

impl<H: IsHeader> std::fmt::Debug for ClientOp<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Backward(tip) => f
                .debug_struct("Backward")
                .field("tip", &(tip.1, PrettyPoint(&tip.0)))
                .finish(),
            Self::Forward(header) => f
                .debug_struct("Forward")
                .field(
                    "header",
                    &(
                        header.block_height(),
                        PrettyPoint(&to_network_point(header.point())),
                    ),
                )
                .field(
                    "tip",
                    &(
                        header.as_tip().1,
                        PrettyPoint(&to_network_point(header.point())),
                    ),
                )
                .finish(),
        }
    }
}

impl<H: IsHeader> ClientOp<H> {
    pub fn tip(&self) -> Tip {
        match self {
            ClientOp::Backward(tip) => tip.clone(),
            ClientOp::Forward(header) => header.as_tip(),
        }
    }
}

pub enum ClientProtocolMsg<H> {
    Op(ClientOp<H>),
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

pub enum ClientMsg<H> {
    /// A new peer has connected to us.
    ///
    /// Our tip is included to get the connection handlers started correctly.
    Peer(PeerServer, Tip),
    /// An operation to be executed on all clients.
    Op(ClientOp<H>),
}

pub async fn client_protocols<H: IsHeader + 'static + Clone + Send>(
    mut cell: ActoCell<ClientProtocolMsg<H>, impl ActoRuntime, anyhow::Result<()>>,
    server: PeerServer,
    store: Arc<dyn ChainStore<H>>,
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

pub enum ChainSyncMsg<H> {
    /// An operation coming in from the ledger
    Op(ClientOp<H>),
    /// A request for data from the client
    ReqNext,
}

async fn chain_sync<H: IsHeader + 'static + Clone + Send>(
    mut cell: ActoCell<ChainSyncMsg<H>, impl ActoRuntime, anyhow::Result<()>>,
    mut server: chainsync::Server<HeaderContent>,
    our_tip: Tip,
    store: Arc<dyn ChainStore<H>>,
) -> anyhow::Result<()> {
    // TODO: do we need to handle validation updates already here in case the client is really slow to ask for intersection?
    let Some(ClientRequest::Intersect(req)) = server.recv_while_idle().await? else {
        // need an intersection point to start
        return Err(ClientError::EarlyRequestNext.into());
    };

    tracing::debug!("finding headers between {:?} and {:?}", our_tip.0, req);
    let Some((catch_up, client_at)) = find_headers_between(store, &our_tip.0, &req) else {
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
                if waiting && let Some(op) = state.next_op() {
                    tracing::debug!("sending op {op:?} to waiting handler");
                    waiting = false;
                    handler.send(Some((op, our_tip.clone())));
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
async fn chain_sync_handler<H: IsHeader + 'static + Clone + Send>(
    mut cell: ActoCell<Option<(ClientOp<H>, Tip)>, impl ActoRuntime>,
    mut server: chainsync::Server<HeaderContent>,
    parent: ActoRef<ChainSyncMsg<H>>,
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
                Some((ClientOp::Forward(header), tip)) => {
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
                        ClientOp::Forward(header) => {
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

pub(super) fn to_header_content<H: IsHeader>(header: H) -> HeaderContent {
    HeaderContent {
        variant: 6,
        byron_prefix: None,
        cbor: to_cbor(&header),
    }
}

enum BlockFetchMsg {}

async fn block_fetch<H: IsHeader>(
    _cell: ActoCell<BlockFetchMsg, impl ActoRuntime>,
    mut server: blockfetch::Server,
    store: Arc<dyn ChainStore<H>>, // TODO: need a block store here
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
