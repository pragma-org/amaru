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

use crate::tx_submission::Blocking;
use crate::tx_submission::tests::MessageEq;
use crate::tx_submission::tx_client_transport::TxClientTransport;
use crate::tx_submission::tx_client_transport::tests::MockClientTransport;
use crate::tx_submission::tx_server_transport::TxServerTransport;
use crate::tx_submission::tx_server_transport::tests::MockServerTransport;
use async_trait::async_trait;
use pallas_network::miniprotocols::txsubmission::{
    EraTxBody, EraTxId, Reply, Request, TxCount, TxIdAndSize,
};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

pub struct MockTransport {
    pub client_transport: MockClientTransport,
    pub server_transport: MockServerTransport,
    pub rx_observe_messages: mpsc::Receiver<MessageEq>,
}

impl MockTransport {
    /// Creates a new mock transport with the specified channel capacity.
    /// This sets up the internal channels for communication between the client and server transports.
    pub fn with_channel_size(channel_capacity: usize) -> Self {
        let (tx_req, rx_req) = mpsc::channel(channel_capacity);
        let (tx_reply, rx_reply) = mpsc::channel(channel_capacity);
        let (tx_observe_messages, rx_observe_messages) = mpsc::channel(channel_capacity);

        let client_transport = MockClientTransport::new(rx_req, tx_observe_messages, tx_reply);
        let server_transport = MockServerTransport::new(rx_reply, tx_req);
        MockTransport::new(client_transport, server_transport, rx_observe_messages)
    }

    /// Creates a new mock transport with the specified client and server transports.
    pub fn new(
        client_transport: MockClientTransport,
        server_transport: MockServerTransport,
        rx_observe_messages: mpsc::Receiver<MessageEq>,
    ) -> Self {
        Self {
            client_transport,
            server_transport,
            rx_observe_messages,
        }
    }
}

#[async_trait]
impl TxServerTransport for MockTransport {
    async fn wait_for_init(&mut self) -> anyhow::Result<()> {
        self.server_transport.wait_for_init().await
    }

    async fn is_done(&self) -> anyhow::Result<bool> {
        Ok(self.server_transport.is_done().await?)
    }

    async fn receive_next_reply(&mut self) -> anyhow::Result<Reply<EraTxId, EraTxBody>> {
        self.server_transport.receive_next_reply().await
    }

    async fn acknowledge_and_request_tx_ids(
        &mut self,
        blocking: Blocking,
        acknowledge: TxCount,
        count: TxCount,
    ) -> anyhow::Result<()> {
        self.server_transport
            .acknowledge_and_request_tx_ids(blocking, acknowledge, count)
            .await
    }

    async fn request_txs(&mut self, txs: Vec<EraTxId>) -> anyhow::Result<()> {
        self.server_transport.request_txs(txs).await
    }
}

#[async_trait]
impl TxServerTransport for Arc<Mutex<MockServerTransport>> {
    async fn wait_for_init(&mut self) -> anyhow::Result<()> {
        self.lock().await.wait_for_init().await
    }

    async fn is_done(&self) -> anyhow::Result<bool> {
        Ok(self.lock().await.is_done().await?)
    }

    async fn receive_next_reply(&mut self) -> anyhow::Result<Reply<EraTxId, EraTxBody>> {
        self.lock().await.receive_next_reply().await
    }

    async fn acknowledge_and_request_tx_ids(
        &mut self,
        blocking: Blocking,
        acknowledge: TxCount,
        count: TxCount,
    ) -> anyhow::Result<()> {
        self.lock()
            .await
            .acknowledge_and_request_tx_ids(blocking, acknowledge, count)
            .await
    }

    async fn request_txs(&mut self, txs: Vec<EraTxId>) -> anyhow::Result<()> {
        self.lock().await.request_txs(txs).await
    }
}

#[async_trait]
impl TxClientTransport for MockTransport {
    async fn send_init(&mut self) -> anyhow::Result<()> {
        self.client_transport.send_init().await
    }

    async fn send_done(&mut self) -> anyhow::Result<()> {
        self.client_transport.send_done().await
    }

    async fn next_request(&mut self) -> anyhow::Result<Request<EraTxId>> {
        self.client_transport.next_request().await
    }

    async fn reply_tx_ids(&mut self, ids: Vec<TxIdAndSize<EraTxId>>) -> anyhow::Result<()> {
        self.client_transport.reply_tx_ids(ids).await
    }

    async fn reply_txs(&mut self, txs: Vec<EraTxBody>) -> anyhow::Result<()> {
        self.client_transport.reply_txs(txs).await
    }
}

#[async_trait]
impl TxClientTransport for Arc<Mutex<MockClientTransport>> {
    async fn send_init(&mut self) -> anyhow::Result<()> {
        self.lock().await.send_init().await
    }

    async fn send_done(&mut self) -> anyhow::Result<()> {
        self.lock().await.send_done().await
    }

    async fn next_request(&mut self) -> anyhow::Result<Request<EraTxId>> {
        self.lock().await.next_request().await
    }

    async fn reply_tx_ids(&mut self, ids: Vec<TxIdAndSize<EraTxId>>) -> anyhow::Result<()> {
        self.lock().await.reply_tx_ids(ids).await
    }

    async fn reply_txs(&mut self, txs: Vec<EraTxBody>) -> anyhow::Result<()> {
        self.lock().await.reply_txs(txs).await
    }
}
