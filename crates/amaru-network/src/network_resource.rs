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

use crate::acto_connection;
use acto::{AcTokioRuntime, ActoRef, ActoRuntime};
use amaru_kernel::tx_submission_events::TxServerRequest;
use amaru_kernel::{
    BlockHeader, Point, TxClientReply,
    connection::{ClientConnectionError, ConnMsg},
    consensus_events::{ChainSyncEvent, Tracked},
    peer::Peer,
};
use amaru_ouroboros::ChainStore;
use amaru_ouroboros_traits::NetworkOperations;
use anyhow::anyhow;
use async_trait::async_trait;
use parking_lot::Mutex;
use std::{collections::BTreeMap, ops::Deref, sync::Arc};
use tokio::sync::{mpsc, oneshot};

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
    pub fn new(
        peers: impl IntoIterator<Item = Peer>,
        rt: &AcTokioRuntime,
        magic: u64,
        store: Arc<dyn ChainStore<BlockHeader>>,
    ) -> Self {
        let (chain_sync_tx, chain_sync_rx) = mpsc::channel(100);
        let (tx_request_tx, tx_request_rx) = mpsc::channel(100);
        let (_tx_reply_tx, tx_reply_rx) = mpsc::channel(100);

        let mut connections = BTreeMap::new();
        let mut tx_client_reply_senders = BTreeMap::new();
        let tx_server_request_senders = BTreeMap::new();

        for peer in peers {
            let (tx_reply_tx, tx_reply_rx) = mpsc::channel(100);
            tx_client_reply_senders.insert(peer.clone(), tx_reply_tx);

            let peer_clone = peer.clone();
            let conn = rt
                .spawn_actor(&format!("conn-{}", peer), |cell| {
                    acto_connection::actor(
                        cell,
                        peer_clone,
                        magic,
                        store.clone(),
                        chain_sync_tx.clone(),
                        tx_request_tx.clone(),
                        tx_reply_rx,
                    )
                })
                .me;

            connections.insert(peer.clone(), conn);
        }

        Self {
            inner: Arc::new(NetworkInner {
                connections,
                chain_sync_rx: tokio::sync::Mutex::new(chain_sync_rx),
                tx_server_request_receiver: tokio::sync::Mutex::new(tx_request_rx),
                tx_client_reply_receiver: tokio::sync::Mutex::new(tx_reply_rx),
                tx_client_reply_senders,
                tx_server_request_senders,
            }),
        }
    }
}

pub struct NetworkInner {
    connections: BTreeMap<Peer, ActoRef<ConnMsg>>,
    chain_sync_rx: tokio::sync::Mutex<mpsc::Receiver<Tracked<ChainSyncEvent>>>,
    tx_server_request_receiver: tokio::sync::Mutex<mpsc::Receiver<TxServerRequest>>,
    tx_client_reply_receiver: tokio::sync::Mutex<mpsc::Receiver<TxClientReply>>,
    tx_client_reply_senders: BTreeMap<Peer, mpsc::Sender<TxClientReply>>,
    tx_server_request_senders: BTreeMap<Peer, mpsc::Sender<TxServerRequest>>,
}

#[async_trait]
impl NetworkOperations for NetworkResource {
    async fn next_sync(&self) -> Tracked<ChainSyncEvent> {
        #[expect(clippy::expect_used)]
        self.inner
            .chain_sync_rx
            .lock()
            .await
            .recv()
            .await
            .expect("upstream funnel will never stop")
    }

    async fn next_tx_request(&self) -> Result<TxServerRequest, ClientConnectionError> {
        if let Some(tx_request) = self
            .inner
            .tx_server_request_receiver
            .lock()
            .await
            .recv()
            .await
        {
            Ok(tx_request)
        } else {
            Err(anyhow!("tx request channel closed").into())
        }
    }

    async fn next_tx_reply(&self) -> Result<TxClientReply, ClientConnectionError> {
        if let Some(tx_reply) = self
            .inner
            .tx_client_reply_receiver
            .lock()
            .await
            .recv()
            .await
        {
            Ok(tx_reply)
        } else {
            Err(anyhow!("tx repyl channel closed").into())
        }
    }

    async fn send_tx_reply(&self, reply: TxClientReply) -> Result<(), ClientConnectionError> {
        if let Some(receiver) = self.inner.tx_client_reply_senders.get(reply.peer()) {
            receiver
                .send(reply)
                .await
                .map_err(|e| ClientConnectionError::new(e.into()))
        } else {
            Ok(())
        }
    }

    async fn send_tx_request(&self, request: TxServerRequest) -> Result<(), ClientConnectionError> {
        if let Some(receiver) = self.inner.tx_server_request_senders.get(request.peer()) {
            receiver
                .send(request)
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
        if let Some(peer) = self.inner.connections.get(peer) {
            peer.send(ConnMsg::FetchBlock(point.clone(), tx.clone()));
        }
        drop(tx);
        // if no sends were made then the drop above ensures that the below errors instead of deadlock
        rx.await
            .map_err(|e| ClientConnectionError::new(e.into()))
            .flatten()
    }

    async fn disconnect(&self, peer: &Peer) {
        if let Some(p) = self.inner.connections.get(peer) {
            p.send_wait(ConnMsg::Disconnect).await;
        }
    }
}
