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
use acto::{AcTokioRuntime, ActoCell, ActoRef, ActoRuntime};
use amaru_kernel::{
    BlockHeader, IsHeader, Point,
    connection::{BlockFetchClientError, ConnMsg},
    consensus_events::ChainSyncEvent,
    peer::Peer,
};
use amaru_ouroboros::ChainStore;
use anyhow::anyhow;
use async_trait::async_trait;
use parking_lot::Mutex;
use pure_stage::{BoxFuture, Effects, ExternalEffect, ExternalEffectAPI, Resources, SendData};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt::Display, ops::Deref, sync::Arc};
use tokio::sync::{Mutex as AsyncMutex, mpsc, oneshot};

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

    fn disconnect(&self, peer: Peer) -> BoxFuture<'_, Result<(), ProcessingFailed>>;
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

    fn disconnect(&self, peer: Peer) -> BoxFuture<'_, Result<(), ProcessingFailed>> {
        let f = self.0.external(DisconnectEffect::new(peer));
        #[allow(clippy::unit_arg)]
        Box::pin(async move { Ok(f.await) })
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
    type Response = ChainSyncEvent;
}

impl ExternalEffect for ChainSyncEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap(async move {
            #[expect(clippy::expect_used)]
            let network = resources
                .get::<NetworkResource>()
                .expect("ChainSyncEffect requires a NetworkResource")
                .clone();
            network.next_sync().await
        })
    }
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
                .get::<NetworkResource>()
                .expect("FetchBlockEffect requires a NetworkResource")
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
                .get::<NetworkResource>()
                .expect("DisconnectEffect requires a NetworkResource")
                .clone();
            network.disconnect(&self.peer).await
        })
    }
}

#[derive(Clone)]
pub struct NetworkResource {
    inner: Arc<NetworkInner>,
}

impl Deref for NetworkResource {
    type Target = NetworkInner;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl NetworkResource {
    pub fn new<F: Future<Output = ()> + Send + 'static>(
        peers: impl IntoIterator<Item = Peer>,
        rt: &AcTokioRuntime,
        magic: u64,
        store: Arc<dyn ChainStore<BlockHeader>>,
        mut actor: impl FnMut(
            ActoCell<ConnMsg, AcTokioRuntime>,
            Peer,
            u64,
            mpsc::Sender<ChainSyncEvent>,
            Arc<dyn ChainStore<BlockHeader>>,
        ) -> F
        + Send
        + 'static,
    ) -> Self {
        let (hd_tx, hd_rx) = mpsc::channel(100);
        let connections = peers
            .into_iter()
            .map(|peer| {
                (
                    peer.clone(),
                    rt.spawn_actor(&format!("conn-{}", peer), |cell| {
                        actor(cell, peer, magic, hd_tx.clone(), store.clone())
                    })
                    .me,
                )
            })
            .collect();
        Self {
            inner: Arc::new(NetworkInner {
                connections,
                hd_rx: AsyncMutex::new(hd_rx),
            }),
        }
    }
}

pub struct NetworkInner {
    connections: BTreeMap<Peer, ActoRef<ConnMsg>>,
    hd_rx: AsyncMutex<mpsc::Receiver<ChainSyncEvent>>,
}

impl NetworkInner {
    pub async fn next_sync(&self) -> ChainSyncEvent {
        #[expect(clippy::expect_used)]
        self.hd_rx
            .lock()
            .await
            .recv()
            .await
            .expect("upstream funnel will never stop")
    }

    pub async fn fetch_block(
        &self,
        peer: &Peer,
        point: Point,
    ) -> Result<Vec<u8>, BlockFetchClientError> {
        let (tx, rx) = oneshot::channel();
        let tx = Arc::new(Mutex::new(Some(tx)));
        if let Some(peer) = self.connections.get(peer) {
            peer.send(ConnMsg::FetchBlock(point.clone(), tx.clone()));
        }
        drop(tx);
        // if no sends were made then the drop above ensures that the below errors instead of deadlock
        rx.await
            .map_err(|e| BlockFetchClientError::new(e.into()))
            .flatten()
    }

    pub async fn disconnect(&self, peer: &Peer) {
        if let Some(p) = self.connections.get(peer) {
            p.send_wait(ConnMsg::Disconnect).await;
        }
    }
}
