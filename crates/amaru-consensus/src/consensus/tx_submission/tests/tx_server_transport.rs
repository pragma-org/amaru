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

use crate::consensus::tx_submission::Blocking;
use crate::consensus::tx_submission::tests::TransportError;
use anyhow::anyhow;
use async_trait::async_trait;
use pallas_network::miniprotocols::txsubmission::{EraTxBody, EraTxId, Message, Reply, TxCount};
use tokio::sync::mpsc::{Receiver, Sender};

/// Abstraction over the tx-submission wire used by the server state machine.
///
/// This lets us unit-test `TxSubmissionServer` without needing a real
/// `AgentChannel` / `TcpStream`. Production code uses the pallas
/// `txsubmission::Server<AgentChannel>` through the adapter below.
#[async_trait]
pub trait TxServerTransport: Send {
    async fn wait_for_init(&mut self) -> Result<(), TransportError>;
    async fn is_done(&self) -> Result<bool, TransportError>;
    async fn receive_next_reply(&mut self) -> Result<Reply<EraTxId, EraTxBody>, TransportError>;

    async fn acknowledge_and_request_tx_ids(
        &mut self,
        blocking: Blocking,
        acknowledge: TxCount,
        count: TxCount,
    ) -> Result<(), TransportError>;

    async fn request_txs(&mut self, txs: Vec<EraTxId>) -> Result<(), TransportError>;
}

pub(crate) struct MockServerTransport {
    // client -> server replies
    rx_reply: Receiver<Message<EraTxId, EraTxBody>>,
    // server -> client messages
    tx_request: Sender<Message<EraTxId, EraTxBody>>,
}

impl MockServerTransport {
    pub(crate) fn new(
        rx_reply: Receiver<Message<EraTxId, EraTxBody>>,
        tx_request: Sender<Message<EraTxId, EraTxBody>>,
    ) -> Self {
        Self {
            rx_reply,
            tx_request,
        }
    }
}

#[async_trait]
impl TxServerTransport for MockServerTransport {
    async fn wait_for_init(&mut self) -> Result<(), TransportError> {
        match self.rx_reply.recv().await {
            Some(Message::Init) => Ok(()),
            _ => Err(TransportError::Other(anyhow!("expected Init message"))),
        }
    }

    async fn is_done(&self) -> Result<bool, TransportError> {
        Ok(false)
    }

    async fn receive_next_reply(&mut self) -> Result<Reply<EraTxId, EraTxBody>, TransportError> {
        match self.rx_reply.recv().await {
            Some(Message::ReplyTxIds(tx_ids)) => Ok(Reply::TxIds(tx_ids)),
            Some(Message::ReplyTxs(txs)) => Ok(Reply::Txs(txs)),
            Some(Message::Done) => Ok(Reply::Done),
            _ => Err(TransportError::Other(anyhow!(
                "expected Reply or Done message"
            ))),
        }
    }

    async fn acknowledge_and_request_tx_ids(
        &mut self,
        blocking: Blocking,
        acknowledge: TxCount,
        count: TxCount,
    ) -> Result<(), TransportError> {
        // Simulate sending a RequestTxIds message to the client
        self.tx_request
            .send(Message::RequestTxIds(blocking.into(), acknowledge, count))
            .await
            .map_err(|e| anyhow!(e))?;
        Ok(())
    }

    async fn request_txs(&mut self, txs: Vec<EraTxId>) -> Result<(), TransportError> {
        // Simulate sending a RequestTxs message to the client
        self.tx_request
            .send(Message::RequestTxs(txs))
            .await
            .map_err(|e| anyhow!(e))?;
        Ok(())
    }
}
