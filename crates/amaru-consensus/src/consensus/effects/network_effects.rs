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

use crate::consensus::{
    errors::{ConsensusError, ProcessingFailed},
    tip::HeaderTip,
};
use amaru_kernel::connection::ClientConnectionError;
use amaru_kernel::{
    BlockHeader, IsHeader, Point, Tx, TxId,
    consensus_events::{ChainSyncEvent, Tracked},
    peer::Peer,
    tx_submission_events::{TxClientReply, TxServerRequest},
};
use amaru_ouroboros::network_operations::ResourceNetworkOperations;
use anyhow::anyhow;
use async_trait::async_trait;
use pure_stage::{BoxFuture, Effects, ExternalEffect, ExternalEffectAPI, Resources, SendData};
use serde::{Deserialize, Serialize};
use std::{fmt::Display, sync::Arc};
use tracing::Span;

/// Network operations available to a stage: fetch block and forward events to peers.
/// This trait can have mock implementations for unit testing a stage.
pub trait NetworkOps {
    fn fetch_block(
        &self,
        peer: Peer,
        point: Point,
    ) -> BoxFuture<'_, Result<Vec<u8>, ConsensusError>>;

    fn send_forward_event(
        &self,
        peer: Peer,
        header: BlockHeader,
    ) -> BoxFuture<'_, Result<(), ProcessingFailed>>;

    fn send_backward_event(
        &self,
        peer: Peer,
        header_tip: HeaderTip,
    ) -> BoxFuture<'_, Result<(), ProcessingFailed>>;

    fn send_tx_ids(
        &self,
        peer: Peer,
        tx_ids: Vec<(TxId, u32)>,
    ) -> BoxFuture<'_, Result<(), ClientConnectionError>>;

    fn request_tx_ids(
        &self,
        peer: Peer,
        ack: u16,
        req: u16,
    ) -> BoxFuture<'_, Result<(), ClientConnectionError>>;

    fn send_txs(
        &self,
        peer: Peer,
        txs: Vec<Tx>,
    ) -> BoxFuture<'_, Result<(), ClientConnectionError>>;

    fn request_txs(
        &self,
        peer: Peer,
        tx_ids: Vec<TxId>,
    ) -> BoxFuture<'_, Result<(), ClientConnectionError>>;

    fn disconnect(&self, peer: Peer) -> BoxFuture<'_, ()>;
}

/// Implementation of NetworkOps using pure_stage::Effects.
pub struct Network<'a, T>(&'a Effects<T>);

impl<'a, T> Network<'a, T> {
    pub fn new(eff: &'a Effects<T>) -> Network<'a, T> {
        Network(eff)
    }
}

impl<T: SendData + Sync> NetworkOps for Network<'_, T> {
    fn fetch_block(
        &self,
        peer: Peer,
        point: Point,
    ) -> BoxFuture<'_, Result<Vec<u8>, ConsensusError>> {
        self.0.external(FetchBlockEffect::new(peer, point))
    }

    fn send_forward_event(
        &self,
        peer: Peer,
        header: BlockHeader,
    ) -> BoxFuture<'_, Result<(), ProcessingFailed>> {
        self.0
            .external(ForwardEventEffect::new(peer, ForwardEvent::Forward(header)))
    }

    fn send_backward_event(
        &self,
        peer: Peer,
        header_tip: HeaderTip,
    ) -> BoxFuture<'_, Result<(), ProcessingFailed>> {
        self.0.external(ForwardEventEffect::new(
            peer,
            ForwardEvent::Backward(header_tip),
        ))
    }

    fn send_tx_ids(
        &self,
        peer: Peer,
        tx_ids: Vec<(TxId, u32)>,
    ) -> BoxFuture<'_, Result<(), ClientConnectionError>> {
        self.0.external(TxIdsReplyEffect::new(peer, tx_ids))
    }

    fn request_tx_ids(
        &self,
        peer: Peer,
        ack: u16,
        req: u16,
    ) -> BoxFuture<'_, Result<(), ClientConnectionError>> {
        self.0.external(TxIdsRequestEffect::new(peer, ack, req))
    }

    fn send_txs(
        &self,
        peer: Peer,
        txs: Vec<Tx>,
    ) -> BoxFuture<'_, Result<(), ClientConnectionError>> {
        self.0.external(TxsReplyEffect::new(peer, txs))
    }

    fn request_txs(
        &self,
        peer: Peer,
        tx_ids: Vec<TxId>,
    ) -> BoxFuture<'_, Result<(), ClientConnectionError>> {
        self.0.external(TxsRequestEffect::new(peer, tx_ids))
    }

    fn disconnect(&self, peer: Peer) -> BoxFuture<'_, ()> {
        self.0.external(DisconnectEffect::new(peer))
    }
}

// EXTERNAL EFFECTS DEFINITIONS

pub type ResourceForwardEventListener = Arc<dyn ForwardEventListener + Send + Sync>;

/// A listener interface for forward events (new headers or rollbacks).
/// These events are either caught for tests or forwarded to downstream peers (see the TcpForwardEventListener implementation).
#[async_trait]
pub trait ForwardEventListener {
    async fn send(&self, event: ForwardEvent) -> anyhow::Result<()>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ForwardEvent {
    Forward(BlockHeader),
    Backward(HeaderTip),
}

impl ForwardEvent {
    pub fn point(&self) -> Point {
        match self {
            ForwardEvent::Forward(header) => header.point(),
            ForwardEvent::Backward(tip) => tip.point(),
        }
    }
}

impl Display for ForwardEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ForwardEvent::Forward(header) => write!(f, "Forward({})", header.point()),
            ForwardEvent::Backward(tip) => write!(f, "Backward({})", tip),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwardEventEffect {
    peer: Peer,
    event: ForwardEvent,
}

impl ForwardEventEffect {
    pub fn new(peer: Peer, event: ForwardEvent) -> Self {
        Self { peer, event }
    }
}

impl ExternalEffect for ForwardEventEffect {
    #[expect(clippy::expect_used)]
    fn run(
        self: Box<Self>,
        resources: Resources,
    ) -> pure_stage::BoxFuture<'static, Box<dyn pure_stage::SendData>> {
        Box::pin(async move {
            let listener = resources
                .get::<ResourceForwardEventListener>()
                .expect("ForwardEventEffect requires a ForwardEventListener")
                .clone();

            let point = self.event.point();
            let result: <Self as ExternalEffectAPI>::Response =
                listener.send(self.event).await.map_err(|e| {
                    ProcessingFailed::new(
                        &self.peer,
                        anyhow!("Cannot send the forward event {}: {e}", &point),
                    )
                });
            Box::new(result) as Box<dyn pure_stage::SendData>
        })
    }
}

impl ExternalEffectAPI for ForwardEventEffect {
    type Response = Result<(), ProcessingFailed>;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ChainSyncEffect;

impl ExternalEffectAPI for ChainSyncEffect {
    type Response = Tracked<ChainSyncEvent>;
}

impl ExternalEffect for ChainSyncEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap(async move {
            #[expect(clippy::expect_used)]
            let network = resources
                .get::<ResourceNetworkOperations>()
                .expect("ChainSyncEffect requires a NetworkOperations")
                .clone();
            network.next_sync().await
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ReceiveTxServerRequestEffect;

impl ExternalEffectAPI for ReceiveTxServerRequestEffect {
    type Response = Result<TxServerRequest, ClientConnectionError>;
}

impl ExternalEffect for ReceiveTxServerRequestEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap(async move {
            #[expect(clippy::expect_used)]
            let network = resources
                .get::<ResourceNetworkOperations>()
                .expect("TxRequestEffect requires a NetworkOperations")
                .clone();
            network.next_tx_request().await
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ReceiveTxClientReplyEffect;

impl ExternalEffectAPI for ReceiveTxClientReplyEffect {
    type Response = Result<TxClientReply, ClientConnectionError>;
}

impl ExternalEffect for ReceiveTxClientReplyEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap(async move {
            #[expect(clippy::expect_used)]
            let network = resources
                .get::<ResourceNetworkOperations>()
                .expect("TxReplyEffect requires a NetworkOperations")
                .clone();
            network.next_tx_reply().await
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxIdsReplyEffect {
    peer: Peer,
    tx_ids: Vec<(TxId, u32)>,
}

impl TxIdsReplyEffect {
    pub fn new(peer: Peer, tx_ids: Vec<(TxId, u32)>) -> Self {
        Self { peer, tx_ids }
    }
}

impl ExternalEffect for TxIdsReplyEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Box::pin(async move {
            let network_operations = resources
                .get::<ResourceNetworkOperations>()
                .expect("TxIdsReplyEffect requires a NetworkOperations instance")
                .clone();

            let result: <Self as ExternalEffectAPI>::Response = network_operations
                .send_tx_reply(TxClientReply::TxIds {
                    peer: self.peer.clone(),
                    tx_ids: self.tx_ids,
                    // TODO: check if this is the correct span to use
                    span: Span::current(),
                })
                .await;
            Box::new(result) as Box<dyn SendData>
        })
    }
}

impl ExternalEffectAPI for TxIdsReplyEffect {
    type Response = Result<(), ClientConnectionError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxIdsRequestEffect {
    peer: Peer,
    ack: u16,
    req: u16,
}

impl TxIdsRequestEffect {
    pub fn new(peer: Peer, ack: u16, req: u16) -> Self {
        Self { peer, ack, req }
    }
}

impl ExternalEffect for TxIdsRequestEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Box::pin(async move {
            let network_operations = resources
                .get::<ResourceNetworkOperations>()
                .expect("TxIdsReplyEffect requires a NetworkOperations instance")
                .clone();

            let result: <Self as ExternalEffectAPI>::Response = network_operations
                .send_tx_request(TxServerRequest::TxIds {
                    peer: self.peer.clone(),
                    ack: self.ack,
                    req: self.req,
                    // TODO: check if this is the correct span to use
                    span: Span::current(),
                })
                .await;
            Box::new(result) as Box<dyn SendData>
        })
    }
}

impl ExternalEffectAPI for TxIdsRequestEffect {
    type Response = Result<(), ClientConnectionError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxsReplyEffect {
    peer: Peer,
    txs: Vec<Tx>,
}

impl TxsReplyEffect {
    pub fn new(peer: Peer, txs: Vec<Tx>) -> Self {
        Self { peer, txs }
    }
}

impl ExternalEffect for TxsReplyEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Box::pin(async move {
            let network_operations = resources
                .get::<ResourceNetworkOperations>()
                .expect("TxsReplyEffect requires a NetworkOperations instance")
                .clone();

            let result: <Self as ExternalEffectAPI>::Response = network_operations
                .send_tx_reply(TxClientReply::Txs {
                    peer: self.peer.clone(),
                    txs: self.txs,
                    // TODO: check if this is the correct span to use
                    span: Span::current(),
                })
                .await;
            Box::new(result) as Box<dyn SendData>
        })
    }
}

impl ExternalEffectAPI for TxsReplyEffect {
    type Response = Result<(), ClientConnectionError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxsRequestEffect {
    peer: Peer,
    tx_ids: Vec<TxId>,
}

impl TxsRequestEffect {
    pub fn new(peer: Peer, tx_ids: Vec<TxId>) -> Self {
        Self { peer, tx_ids }
    }
}

impl ExternalEffect for TxsRequestEffect {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Box::pin(async move {
            let network_operations = resources
                .get::<ResourceNetworkOperations>()
                .expect("TxsReplyEffect requires a NetworkOperations instance")
                .clone();

            let result: <Self as ExternalEffectAPI>::Response = network_operations
                .send_tx_request(TxServerRequest::Txs {
                    peer: self.peer.clone(),
                    tx_ids: self.tx_ids,
                    // TODO: check if this is the correct span to use
                    span: Span::current(),
                })
                .await;
            Box::new(result) as Box<dyn SendData>
        })
    }
}

impl ExternalEffectAPI for TxsRequestEffect {
    type Response = Result<(), ClientConnectionError>;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FetchBlockEffect {
    peer: Peer,
    point: Point,
}

impl ExternalEffectAPI for FetchBlockEffect {
    type Response = Result<Vec<u8>, ConsensusError>;
}

impl FetchBlockEffect {
    pub fn new(peer: Peer, point: Point) -> Self {
        Self { peer, point }
    }
}

impl ExternalEffect for FetchBlockEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap(async move {
            #[expect(clippy::expect_used)]
            let network = resources
                .get::<ResourceNetworkOperations>()
                .expect("FetchBlockEffect requires a NetworkOperations")
                .clone();
            let point = self.point.clone();
            network
                .fetch_block(&self.peer, self.point)
                .await
                .map_err(|err| {
                    tracing::warn!(%point, %err, "fetch block failed");
                    ConsensusError::FetchBlockFailed(point)
                })
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DisconnectEffect {
    peer: Peer,
}

impl ExternalEffectAPI for DisconnectEffect {
    type Response = ();
}

impl DisconnectEffect {
    pub fn new(peer: Peer) -> Self {
        Self { peer }
    }
}

impl ExternalEffect for DisconnectEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap(async move {
            #[expect(clippy::expect_used)]
            let network = resources
                .get::<ResourceNetworkOperations>()
                .expect("DisconnectEffect requires a NetworkOperations")
                .clone();
            network.disconnect(&self.peer).await
        })
    }
}
