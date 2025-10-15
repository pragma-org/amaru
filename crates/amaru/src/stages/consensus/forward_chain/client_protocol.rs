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
use acto::{ActoCell, ActoInput, ActoRef, ActoRuntime};
use amaru_consensus::ChainStore;
use amaru_kernel::{Hash, to_cbor};
use amaru_ouroboros_traits::IsHeader;
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
pub enum ClientOp {
    /// the tip to go back to
    Backward(Tip),

    /// the header to go forward to and the tip we will be at after sending this header
    Forward(Tip),
}

impl PartialEq for ClientOp {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Backward(l0), Self::Backward(r0)) => (&l0.0, l0.1) == (&r0.0, r0.1),
            (Self::Forward(l0), Self::Forward(r0)) => l0.0 == r0.0 && l0.1 == r0.1,
            _ => false,
        }
    }
}

impl Eq for ClientOp {}

impl std::fmt::Debug for ClientOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Backward(tip) => f
                .debug_struct("Backward")
                .field("tip", &(tip.1, PrettyPoint(&tip.0)))
                .finish(),
            Self::Forward(point) => f.debug_struct("Forward").field("point", point).finish(),
        }
    }
}

impl ClientOp {
    pub fn tip(&self) -> Tip {
        match self {
            ClientOp::Backward(tip) => tip.clone(),
            ClientOp::Forward(tip) => tip.clone(),
        }
    }
}
pub enum ClientProtocolMsg {
    Op(ClientOp),
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

pub(crate) fn hash_point(point: &Point) -> Hash<32> {
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
    Op(ClientOp),
}

pub async fn client_protocols<H: IsHeader + 'static + Clone + Send>(
    mut cell: ActoCell<ClientProtocolMsg, impl ActoRuntime, anyhow::Result<()>>,
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

pub enum ChainSyncMsg {
    /// An operation coming in from the ledger
    Op(ClientOp),

    /// A request for data from the client
    ReqNext,
}

async fn chain_sync<H: IsHeader + 'static + Clone + Send>(
    mut cell: ActoCell<ChainSyncMsg, impl ActoRuntime, anyhow::Result<()>>,
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
    let Some((catch_up, intersection_point)) =
        find_headers_between(store.clone(), &our_tip.0, &req)
    else {
        tracing::debug!("no intersection found");
        server.send_intersect_not_found(our_tip).await?;
        return Err(ClientError::NoIntersection.into());
    };
    tracing::debug!("intersection found: {intersection_point:?}");
    server
        .send_intersect_found(intersection_point.0.clone(), our_tip.clone())
        .await?;

    let parent = cell.me();
    let chain_store = store.clone();
    let handler = cell.spawn_supervised("chainsync_handler", move |cell| {
        chain_sync_handler(cell, server, chain_store, parent)
    });

    let mut state = ClientState::new(catch_up.into());
    let mut our_tip = our_tip;
    let mut waiting = false;
    loop {
        let input = cell.recv().await;
        match input {
            ActoInput::Message(ChainSyncMsg::Op(op)) => {
                tracing::trace!("got op {op:?}");
                our_tip = op.tip();
                state.add_op(op);
                if waiting && let Some(op) = state.next_op() {
                    tracing::trace!("sending op {op:?} to waiting handler");
                    waiting = false;
                    handler.send(Some((op, our_tip.clone())));
                }
            }
            ActoInput::Message(ChainSyncMsg::ReqNext) => {
                tracing::trace!("got req next");
                if let Some(op) = state.next_op() {
                    tracing::trace!("sending op {op:?} to handler");
                    handler.send(Some((op, our_tip.clone())));
                } else {
                    tracing::trace!("sending await reply");
                    handler.send(None);
                    waiting = true;
                }
            }
            ActoInput::NoMoreSenders => {
                tracing::trace!("no more senders");
                return Ok(());
            }
            ActoInput::Supervision { result, .. } => {
                tracing::trace!("supervision result: {result:?}");
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
    mut cell: ActoCell<Option<(ClientOp, Tip)>, impl ActoRuntime>,
    mut server: chainsync::Server<HeaderContent>,
    store: Arc<dyn ChainStore<H>>,
    parent: ActoRef<ChainSyncMsg>,
) -> anyhow::Result<()> {
    // Handle reply to client
    async fn handle_client_op<H: IsHeader + 'static + Clone + Send>(
        server: &mut chainsync::Server<HeaderContent>,
        store: &Arc<dyn ChainStore<H>>,
        op: ClientOp,
        our_tip: Tip,
    ) -> Result<(), anyhow::Error> {
        match op {
            ClientOp::Forward(tip) => {
                if let Some(header) = store.load_header(&hash_point(&tip.0)) {
                    tracing::trace!(?tip, "sending roll forward");
                    server
                        .send_roll_forward(to_header_content(header), our_tip)
                        .await?;
                } else {
                    return Err(ClientError::HandlerFailure(format!(
                        "failed to load header for point {:?}, definitely a bug!",
                        tip
                    ))
                    .into());
                }
            }
            ClientOp::Backward(point) => {
                tracing::trace!(?point, "sending roll backward");
                server.send_roll_backward(point.0, our_tip).await?;
            }
        };
        Ok(())
    }

    loop {
        let Some(req) = server.recv_while_idle().await? else {
            tracing::trace!("client terminated");
            return Err(ClientError::ClientTerminated.into());
        };

        // FIXME: why do we require the client to RequestNext? We are supposed to be in
        // StIdle state and according to ChainSync protocol the client should always be
        // able to either RequestNext or FindIntersect.
        if !matches!(req, ClientRequest::RequestNext) {
            tracing::warn!("late intersection");
            return Err(ClientError::LateIntersection.into());
        };
        tracing::trace!("got req next");
        parent.send(ChainSyncMsg::ReqNext);

        if let ActoInput::Message(op) = cell.recv().await {
            match op {
                Some((op, tip)) => handle_client_op(&mut server, &store, op, tip).await?,
                None => {
                    tracing::trace!("sending await reply");
                    server.send_await_reply().await?;
                    let ActoInput::Message(Some((op, tip))) = cell.recv().await else {
                        return Ok(());
                    };
                    handle_client_op(&mut server, &store, op, tip).await?
                }
            }
        } else {
            tracing::trace!("parent terminated");
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
