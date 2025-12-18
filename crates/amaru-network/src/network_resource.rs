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
use amaru_kernel::{
    BlockHeader, Point,
    connection::{BlockFetchClientError, ConnMsg},
    consensus_events::{ChainSyncEvent, Tracked},
    peer::Peer,
};
use amaru_ouroboros::ChainStore;
use amaru_ouroboros_traits::NetworkOperations;
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
        let (hd_tx, hd_rx) = mpsc::channel(100);
        let connections = peers
            .into_iter()
            .map(|peer| {
                (
                    peer.clone(),
                    rt.spawn_actor(&format!("conn-{}", peer), |cell| {
                        acto_connection::actor(cell, peer, magic, hd_tx.clone(), store.clone())
                    })
                    .me,
                )
            })
            .collect();
        Self {
            inner: Arc::new(NetworkInner {
                connections,
                hd_rx: tokio::sync::Mutex::new(hd_rx),
            }),
        }
    }
}

pub struct NetworkInner {
    connections: BTreeMap<Peer, ActoRef<ConnMsg>>,
    hd_rx: tokio::sync::Mutex<mpsc::Receiver<Tracked<ChainSyncEvent>>>,
}

#[async_trait::async_trait]
impl NetworkOperations for NetworkResource {
    async fn next_sync(&self) -> Tracked<ChainSyncEvent> {
        #[expect(clippy::expect_used)]
        self.inner
            .hd_rx
            .lock()
            .await
            .recv()
            .await
            .expect("upstream funnel will never stop")
    }

    async fn fetch_block(
        &self,
        peer: &Peer,
        point: Point,
    ) -> Result<Vec<u8>, BlockFetchClientError> {
        let (tx, rx) = oneshot::channel();
        let tx = Arc::new(Mutex::new(Some(tx)));
        if let Some(peer) = self.inner.connections.get(peer) {
            peer.send(ConnMsg::FetchBlock(point, tx.clone()));
        }
        drop(tx);
        // if no sends were made then the drop above ensures that the below errors instead of deadlock
        rx.await
            .map_err(|e| BlockFetchClientError::new(e.into()))
            .flatten()
    }

    async fn disconnect(&self, peer: &Peer) {
        if let Some(p) = self.inner.connections.get(peer) {
            p.send_wait(ConnMsg::Disconnect).await;
        }
    }
}
