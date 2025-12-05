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

use crate::simulator::Envelope;
use crate::simulator::bytes::Bytes;
use crate::sync::ChainSyncMessage;
use amaru_consensus::NetworkOperations;
use amaru_consensus::network_operations::ForwardEvent;
use amaru_kernel::connection::ClientConnectionError;
use amaru_kernel::consensus_events::{ChainSyncEvent, Tracked};
use amaru_kernel::peer::Peer;
use amaru_kernel::{IsHeader, Point, TxClientReply, TxServerRequest, to_cbor};
use async_trait::async_trait;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::mpsc;

pub struct MockNetworkResource {
    node_id: String,
    number_of_downstream_peers: u8,
    sender: mpsc::Sender<Envelope<ChainSyncMessage>>,
    msg_id: Arc<AtomicU64>,
}

impl MockNetworkResource {
    pub fn new(
        node_id: String,
        number_of_downstream_peers: u8,
        sender: mpsc::Sender<Envelope<ChainSyncMessage>>,
    ) -> Self {
        Self {
            node_id,
            number_of_downstream_peers,
            msg_id: Arc::new(AtomicU64::new(0)),
            sender,
        }
    }
}

#[async_trait]
impl NetworkOperations for MockNetworkResource {
    async fn next_sync(&self) -> Tracked<ChainSyncEvent> {
        todo!("next_sync")
    }

    async fn next_tx_request(&self) -> Result<TxServerRequest, ClientConnectionError> {
        todo!("next_tx_request")
    }

    async fn next_tx_reply(&self) -> Result<TxClientReply, ClientConnectionError> {
        todo!("next_tx_reply")
    }

    async fn send_tx_reply(&self, _reply: TxClientReply) -> Result<(), ClientConnectionError> {
        todo!("send_tx_reply")
    }

    async fn send_tx_request(
        &self,
        _request: TxServerRequest,
    ) -> Result<(), ClientConnectionError> {
        todo!("send_tx_request")
    }

    async fn fetch_block(
        &self,
        _peer: &Peer,
        _point: Point,
    ) -> Result<Vec<u8>, ClientConnectionError> {
        todo!("fetch_block")
    }

    async fn send(&self, event: ForwardEvent) -> anyhow::Result<()> {
        fn message(event: &ForwardEvent, msg_id: u64) -> ChainSyncMessage {
            match event {
                ForwardEvent::Forward(header) => ChainSyncMessage::Fwd {
                    msg_id,
                    slot: header.point().slot_or_default(),
                    hash: Bytes {
                        bytes: header.hash().to_vec(),
                    },
                    header: Bytes {
                        bytes: to_cbor(&header),
                    },
                },
                ForwardEvent::Backward(tip) => ChainSyncMessage::Bck {
                    msg_id,
                    slot: tip.point().slot_or_default(),
                    hash: Bytes {
                        bytes: tip.hash().as_slice().to_vec(),
                    },
                },
            }
        }

        // This allocates a range of message ids from
        // self.msg_id to self.msg_id + number_of_downstream_peers
        let base_msg_id = self
            .msg_id
            .fetch_add(self.number_of_downstream_peers as u64, Ordering::Relaxed);

        for i in 1..=self.number_of_downstream_peers {
            let dest = format!("c{}", i);
            let msg_id = base_msg_id + i as u64;
            let envelope = Envelope {
                src: self.node_id.clone(),
                dest,
                body: message(&event, msg_id),
            };
            self.sender.send(envelope).await?;
        }
        Ok(())
    }

    async fn disconnect(&self, _peer: &Peer) {
        todo!("disconnect")
    }
}
