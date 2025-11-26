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

use async_trait::async_trait;
use pallas_network::miniprotocols::txsubmission;
use pallas_network::miniprotocols::txsubmission::{EraTxBody, EraTxId, Request, TxIdAndSize};
use pallas_network::multiplexer::AgentChannel;

/// Abstraction over the tx-submission wire used by the client state machine.
///
/// This lets us unit-test `TxSubmissionClient` without needing a real
/// `AgentChannel` / `TcpStream`. Production code uses the pallas
/// `txsubmission::Client<AgentChannel>` through the adapter below.
#[async_trait]
pub trait TxClientTransport: Send {
    async fn send_init(&mut self) -> anyhow::Result<()>;
    async fn send_done(&mut self) -> anyhow::Result<()>;

    async fn next_request(&mut self) -> anyhow::Result<Request<EraTxId>>;

    async fn reply_tx_ids(&mut self, ids: Vec<TxIdAndSize<EraTxId>>) -> anyhow::Result<()>;
    async fn reply_txs(&mut self, txs: Vec<EraTxBody>) -> anyhow::Result<()>;
}

/// Production adapter around pallas' txsubmission client.
pub struct PallasTxClientTransport {
    inner: txsubmission::Client,
}

impl PallasTxClientTransport {
    pub fn new(agent_channel: AgentChannel) -> Self {
        Self {
            inner: txsubmission::Client::new(agent_channel),
        }
    }
}

#[async_trait]
impl TxClientTransport for PallasTxClientTransport {
    async fn send_init(&mut self) -> anyhow::Result<()> {
        Ok(self.inner.send_init().await?)
    }

    async fn send_done(&mut self) -> anyhow::Result<()> {
        Ok(self.inner.send_done().await?)
    }

    async fn next_request(&mut self) -> anyhow::Result<Request<EraTxId>> {
        Ok(self.inner.next_request().await?)
    }

    async fn reply_tx_ids(&mut self, ids: Vec<TxIdAndSize<EraTxId>>) -> anyhow::Result<()> {
        Ok(self.inner.reply_tx_ids(ids).await?)
    }

    async fn reply_txs(&mut self, txs: Vec<EraTxBody>) -> anyhow::Result<()> {
        Ok(self.inner.reply_txs(txs).await?)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use crate::tx_submission::tests::MessageEq;
    use crate::tx_submission::tx_client_transport::TxClientTransport;
    use async_trait::async_trait;
    use pallas_network::miniprotocols::txsubmission::{
        EraTxBody, EraTxId, Message, Request, TxIdAndSize,
    };
    use tokio::sync::mpsc::{Receiver, Sender};

    pub(crate) struct MockClientTransport {
        // server -> client messages
        rx_req: Receiver<Message<EraTxId, EraTxBody>>,
        // for external inspection of sent messages
        tx_req: Sender<MessageEq>,
        // client -> server messages
        tx_reply: Sender<Message<EraTxId, EraTxBody>>,
    }

    impl MockClientTransport {
        pub(crate) fn new(
            rx_req: Receiver<Message<EraTxId, EraTxBody>>,
            tx_req: Sender<MessageEq>,
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
        async fn send_init(&mut self) -> anyhow::Result<()> {
            self.tx_reply.send(Message::Init).await?;
            Ok(())
        }

        async fn send_done(&mut self) -> anyhow::Result<()> {
            self.tx_reply.send(Message::Done).await?;
            Ok(())
        }

        async fn next_request(&mut self) -> anyhow::Result<Request<EraTxId>> {
            let received = self.rx_req.recv().await;
            if let Some(received) = &received {
                self.tx_req.send(received.into()).await?;
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
                Some(other) => Err(anyhow::anyhow!(
                    "Unexpected message received in MockClientTransport: {:?}",
                    other
                )),
                None => Err(anyhow::anyhow!("mock closed")),
            }
        }

        async fn reply_tx_ids(&mut self, ids: Vec<TxIdAndSize<EraTxId>>) -> anyhow::Result<()> {
            self.tx_reply.send(Message::ReplyTxIds(ids)).await?;
            Ok(())
        }

        async fn reply_txs(&mut self, txs: Vec<EraTxBody>) -> anyhow::Result<()> {
            self.tx_reply.send(Message::ReplyTxs(txs)).await?;
            Ok(())
        }
    }
}
