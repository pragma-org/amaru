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

use anyhow::anyhow;
use async_trait::async_trait;
use pallas_network::miniprotocols::txsubmission::{
    EraTxBody, EraTxId, Error, Message, Request, TxIdAndSize,
};
use tokio::sync::mpsc::{Receiver, Sender};

/// Abstraction over the tx-submission wire used by the client state machine.
///
/// This lets us unit test the `TxSubmissionClient` without needing a real `TcpStream`.
#[async_trait]
pub trait TxClientTransport: Send {
    async fn send_init(&mut self) -> Result<(), TransportError>;
    async fn send_done(&mut self) -> Result<(), TransportError>;

    async fn next_request(&mut self) -> Result<Request<EraTxId>, TransportError>;

    async fn reply_tx_ids(&mut self, ids: Vec<TxIdAndSize<EraTxId>>) -> Result<(), TransportError>;
    async fn reply_txs(&mut self, txs: Vec<EraTxBody>) -> Result<(), TransportError>;
}

/// For now the transport error is either a pallas Error or a generic anyhow error.
#[derive(thiserror::Error, Debug)]
pub enum TransportError {
    #[error("pallas error: {0}")]
    PallasError(#[from] Error),
    #[error("other error: {0}")]
    Other(#[from] anyhow::Error),
}

pub struct MockClientTransport {
    // server -> client messages
    rx_req: Receiver<Message<EraTxId, EraTxBody>>,
    // for external inspection of sent messages
    tx_req: Sender<Message<EraTxId, EraTxBody>>,
    // client -> server messages
    tx_reply: Sender<Message<EraTxId, EraTxBody>>,
}

impl MockClientTransport {
    pub fn new(
        rx_req: Receiver<Message<EraTxId, EraTxBody>>,
        tx_req: Sender<Message<EraTxId, EraTxBody>>,
        tx_reply: Sender<Message<EraTxId, EraTxBody>>,
    ) -> Self {
        Self {
            rx_req,
            tx_req,
            tx_reply,
        }
    }
}

#[async_trait]
impl TxClientTransport for MockClientTransport {
    async fn send_init(&mut self) -> Result<(), TransportError> {
        self.tx_reply
            .send(Message::Init)
            .await
            .map_err(|e| anyhow!(e))?;
        Ok(())
    }

    async fn send_done(&mut self) -> Result<(), TransportError> {
        self.tx_reply
            .send(Message::Done)
            .await
            .map_err(|e| anyhow!(e))?;
        Ok(())
    }

    async fn next_request(&mut self) -> Result<Request<EraTxId>, TransportError> {
        let received = self.rx_req.recv().await;
        if let Some(received) = &received {
            self.tx_req
                .send(received.clone())
                .await
                .map_err(|e| anyhow!(e))?;
        };

        match received {
            Some(Message::RequestTxIds(blocking, ack, req)) => {
                if blocking {
                    Ok(Request::TxIds(ack, req))
                } else {
                    Ok(Request::TxIdsNonBlocking(ack, req))
                }
            }
            Some(Message::RequestTxs(x)) => Ok(Request::Txs(x)),
            Some(other) => Err(TransportError::Other(anyhow::anyhow!(
                "Unexpected message received in MockClientTransport: {:?}",
                other
            ))),
            None => Err(TransportError::Other(anyhow::anyhow!("mock closed"))),
        }
    }

    async fn reply_tx_ids(&mut self, ids: Vec<TxIdAndSize<EraTxId>>) -> Result<(), TransportError> {
        self.tx_reply
            .send(Message::ReplyTxIds(ids))
            .await
            .map_err(|e| anyhow!(e))?;
        Ok(())
    }

    async fn reply_txs(&mut self, txs: Vec<EraTxBody>) -> Result<(), TransportError> {
        self.tx_reply
            .send(Message::ReplyTxs(txs))
            .await
            .map_err(|e| anyhow!(e))?;
        Ok(())
    }
}
