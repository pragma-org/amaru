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

use crate::server::downstream_server::DownstreamServer;
use crate::upstream_connection;
use acto::{AcTokioRuntime, ActoRef, ActoRuntime};
use amaru_kernel::is_header::HeaderTip;
use amaru_kernel::tx_submission_events::TxServerRequest;
use amaru_kernel::{
    BlockHeader, Point, TxClientReply,
    connection::{ClientConnectionError, ConnMsg},
    consensus_events::{ChainSyncEvent, Tracked},
    peer::Peer,
};
use amaru_ouroboros::ChainStore;
use amaru_ouroboros_traits::NetworkOperations;
use amaru_ouroboros_traits::network_operations::ForwardEvent;
use anyhow::anyhow;
use async_trait::async_trait;
use parking_lot::Mutex;
use std::{collections::BTreeMap, ops::Deref, sync::Arc};
use tokio::sync::{mpsc, oneshot};

#[derive(Clone)]
pub struct NetworkResource {
    upstream_client: Arc<UpstreamClient>,
    downstream_server: Arc<DownstreamServer>,
}

impl Deref for NetworkResource {
    type Target = UpstreamClient;

    fn deref(&self) -> &Self::Target {
        &self.upstream_client
    }
}

impl NetworkResource {
    pub async fn new(
        rt: &AcTokioRuntime,
        chain_store: Arc<dyn ChainStore<BlockHeader>>,
        upstream_peers: impl IntoIterator<Item = Peer>,
        max_downstream_peers: usize,
        network_magic: u64,
        listen_address: String,
        our_tip: HeaderTip,
    ) -> anyhow::Result<Self> {
        let upstream_client =
            UpstreamClient::new(rt, upstream_peers, chain_store.clone(), network_magic);

        let downstream_server = DownstreamServer::new(
            chain_store.clone(),
            listen_address.clone(),
            network_magic,
            max_downstream_peers,
            our_tip.clone(),
        )
        .await?;

        Ok(Self {
            upstream_client: Arc::new(upstream_client),
            downstream_server: Arc::new(downstream_server),
        })
    }
}

pub struct UpstreamClient {
    connections: BTreeMap<Peer, ActoRef<ConnMsg>>,
    /// Channel receiving all chain sync events from upstream connections
    chain_sync_event_receiver: tokio::sync::Mutex<mpsc::Receiver<Tracked<ChainSyncEvent>>>,
    /// Channel receiving all tx server requests from upstream connections
    /// Each request carries the peer it originated from.
    tx_server_request_receiver: tokio::sync::Mutex<mpsc::Receiver<TxServerRequest>>,
    /// Channels sending client replies to upstream peers
    tx_client_reply_senders: BTreeMap<Peer, mpsc::Sender<TxClientReply>>,
}

impl UpstreamClient {
    pub fn new(
        rt: &AcTokioRuntime,
        upstream_peers: impl IntoIterator<Item = Peer>,
        chain_store: Arc<dyn ChainStore<BlockHeader>>,
        network_magic: u64,
    ) -> Self {
        let (chain_sync_tx, chain_sync_rx) = mpsc::channel(100);
        let (tx_request_tx, tx_request_rx) = mpsc::channel(100);

        let mut connections = BTreeMap::new();
        let mut tx_client_reply_senders = BTreeMap::new();

        for peer in upstream_peers {
            let (tx_reply_tx, tx_reply_rx) = mpsc::channel(100);
            tx_client_reply_senders.insert(peer.clone(), tx_reply_tx);

            let peer_clone = peer.clone();
            let conn = rt
                .spawn_actor(&format!("conn-{}", peer), |cell| {
                    upstream_connection::actor(
                        cell,
                        peer_clone,
                        network_magic,
                        chain_store.clone(),
                        chain_sync_tx.clone(),
                        tx_request_tx.clone(),
                        tx_reply_rx,
                    )
                })
                .me;

            connections.insert(peer.clone(), conn);
        }
        Self {
            connections,
            chain_sync_event_receiver: tokio::sync::Mutex::new(chain_sync_rx),
            tx_server_request_receiver: tokio::sync::Mutex::new(tx_request_rx),
            tx_client_reply_senders,
        }
    }

    async fn next_sync(&self) -> Tracked<ChainSyncEvent> {
        #[expect(clippy::expect_used)]
        self.chain_sync_event_receiver
            .lock()
            .await
            .recv()
            .await
            .expect("upstream funnel will never stop")
    }

    async fn next_tx_request(&self) -> Result<TxServerRequest, ClientConnectionError> {
        if let Some(tx_request) = self.tx_server_request_receiver.lock().await.recv().await {
            Ok(tx_request)
        } else {
            Err(anyhow!("tx request channel closed").into())
        }
    }

    async fn send_tx_reply(&self, reply: TxClientReply) -> Result<(), ClientConnectionError> {
        if let Some(receiver) = self.tx_client_reply_senders.get(reply.peer()) {
            receiver
                .send(reply)
                .await
                .map_err(|e| ClientConnectionError::new(e.into()))
        } else {
            Ok(())
        }
    }

    async fn fetch_block(
        &self,
        peer: &Peer,
        point: Point,
    ) -> Result<Vec<u8>, ClientConnectionError> {
        let (tx, rx) = oneshot::channel();
        let tx = Arc::new(Mutex::new(Some(tx)));
        if let Some(peer) = self.connections.get(peer) {
            peer.send(ConnMsg::FetchBlock(point.clone(), tx.clone()));
        }
        drop(tx);
        // if no sends were made then the drop above ensures that the below errors instead of deadlock
        rx.await
            .map_err(|e| ClientConnectionError::new(e.into()))
            .flatten()
    }

    async fn disconnect(&self, peer: &Peer) {
        if let Some(p) = self.connections.get(peer) {
            p.send_wait(ConnMsg::Disconnect).await;
        }
    }
}

#[async_trait]
impl NetworkOperations for NetworkResource {
    async fn next_sync(&self) -> Tracked<ChainSyncEvent> {
        self.upstream_client.next_sync().await
    }

    async fn next_tx_request(&self) -> Result<TxServerRequest, ClientConnectionError> {
        self.upstream_client.next_tx_request().await
    }

    async fn send_tx_reply(&self, reply: TxClientReply) -> Result<(), ClientConnectionError> {
        self.upstream_client.send_tx_reply(reply).await
    }

    async fn fetch_block(
        &self,
        peer: &Peer,
        point: Point,
    ) -> Result<Vec<u8>, ClientConnectionError> {
        self.upstream_client.fetch_block(peer, point).await
    }

    async fn disconnect(&self, peer: &Peer) {
        self.upstream_client.disconnect(peer).await
    }

    async fn next_tx_reply(&self) -> Result<TxClientReply, ClientConnectionError> {
        self.downstream_server.next_tx_reply().await
    }

    async fn send_tx_request(&self, request: TxServerRequest) -> Result<(), ClientConnectionError> {
        self.downstream_server.send_tx_request(request).await
    }

    async fn send(&self, event: ForwardEvent) -> anyhow::Result<()> {
        self.downstream_server.send(event).await
    }
}
