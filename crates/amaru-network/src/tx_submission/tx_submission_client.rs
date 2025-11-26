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

use crate::tx_submission::tx_client_transport::{PallasTxClientTransport, TxClientTransport};
use amaru_kernel::peer::Peer;
use amaru_kernel::{Hash, to_cbor};
use amaru_ouroboros_traits::{Mempool, MempoolSeqNo, TxId};
use minicbor::{CborLen, Encode};
use pallas_network::miniprotocols::txsubmission::{EraTxBody, EraTxId, Request, TxIdAndSize};
use pallas_network::multiplexer::AgentChannel;
use pallas_traverse::Era;
use std::collections::VecDeque;
use std::sync::Arc;
use tracing::{debug, info};

/// Tx submission client state machine for a given peer.
///
/// The `window` field tracks the transactions that have been advertised to the peer.
/// The `last_seq` field tracks the last sequence number that has been acknowledged by the peer.
///
pub struct TxSubmissionClient<Tx: Send + Sync + 'static> {
    /// Mempool to pull transactions from.
    mempool: Arc<dyn Mempool<Tx>>,
    /// Peer we are serving.
    peer: Peer,
    /// What we’ve already advertised but has not yet been fully acked.
    window: VecDeque<(TxId, MempoolSeqNo)>,
    /// Last seq_no we have ever pulled from the mempool for this peer.
    /// None if we have not pulled anything yet.
    last_seq: Option<MempoolSeqNo>,
}

impl<Tx: Encode<()> + CborLen<()> + Send + Sync + 'static> TxSubmissionClient<Tx> {
    /// Create a new tx submission client state machine for the given mempool and peer.
    pub fn new(mempool: Arc<dyn Mempool<Tx>>, peer: Peer) -> Self {
        Self {
            mempool: mempool.clone(),
            peer,
            window: VecDeque::new(),
            last_seq: None,
        }
    }

    /// Start the tx submission client state machine over the given agent channel.
    /// This function drives the state machine until completion.
    pub async fn start_client(&mut self, agent_channel: AgentChannel) -> anyhow::Result<()> {
        let transport = PallasTxClientTransport::new(agent_channel);
        self.start_client_with_transport(transport).await
    }

    /// Core state machine, parameterized over a transport for testability.
    pub async fn start_client_with_transport<T: TxClientTransport>(
        &mut self,
        mut transport: T,
    ) -> anyhow::Result<()> {
        transport.send_init().await?;
        info!(peer = %self.peer,
            "Started tx submission client for peer {}",
            self.peer
        );
        loop {
            let request = match transport.next_request().await {
                Ok(r) => r,
                Err(_) => {
                    debug!(peer = %self.peer,
                        "Error receiving next request from peer {}, terminating tx submission client",
                        self.peer
                    );
                    break;
                }
            };
            match request {
                Request::TxIds(acknowledged, required_next) => {
                    if !self
                        .mempool
                        .wait_for_at_least(self.next_seq().add(required_next as u64))
                        .await
                    {
                        transport.send_done().await?;
                        break;
                    }
                    self.send_next_tx_ids(&mut transport, acknowledged, required_next)
                        .await?;
                }
                Request::TxIdsNonBlocking(acknowledged, required_next) => {
                    self.send_next_tx_ids(&mut transport, acknowledged, required_next)
                        .await?;
                }
                Request::Txs(ids) => {
                    let ids: Vec<TxId> = ids
                        .into_iter()
                        .map(|tx_id| TxId::new(Hash::from(tx_id.1.as_slice())))
                        .collect();
                    let txs = self.mempool.get_txs_for_ids(ids.as_slice());
                    transport
                        .reply_txs(
                            txs.into_iter()
                                .map(|tx| {
                                    let tx_body = to_cbor(&*tx);
                                    // TODO: see how to handle multiple eras
                                    EraTxBody(Era::Conway.into(), tx_body)
                                })
                                .collect::<Vec<_>>(),
                        )
                        .await?;
                }
            }
        }
        Ok(())
    }

    /// Take notice of the acknowledged transactions, and send the next batch of tx ids.
    async fn send_next_tx_ids<T: TxClientTransport>(
        &mut self,
        transport: &mut T,
        acknowledged: u16,
        required_next: u16,
    ) -> anyhow::Result<()> {
        self.discard(acknowledged);
        let tx_ids = self.mempool.tx_ids_since(self.next_seq(), required_next);
        transport
            .reply_tx_ids(
                tx_ids
                    .iter()
                    .map(|(tx_id, tx_size, _)| {
                        TxIdAndSize(EraTxId(Era::Conway.into(), tx_id.to_vec()), *tx_size)
                    })
                    .collect(),
            )
            .await?;
        self.update(tx_ids);
        Ok(())
    }

    /// We discard up to 'acknowledged' transactions from our window.
    fn discard(&mut self, acknowledged: u16) {
        if self.window.len() >= acknowledged as usize {
            self.window = self.window.drain(acknowledged as usize..).collect();
        }
    }

    /// We update our window with tx ids retrieved from the mempool and just sent to the server.
    fn update(&mut self, tx_ids: Vec<(TxId, u32, MempoolSeqNo)>) {
        for (tx_id, _size, seq_no) in tx_ids {
            self.window.push_back((tx_id, seq_no));
            self.last_seq = Some(seq_no);
        }
    }

    /// Compute the next sequence number to use when pulling from the mempool.
    fn next_seq(&self) -> MempoolSeqNo {
        match self.last_seq {
            Some(seq) => seq.next(),
            None => MempoolSeqNo(0),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::tx_submission::tests::{Tx, assert_next_message, assert_tx_bodies_reply, assert_tx_ids_reply, new_era_tx_id, new_era_tx_body};
    use crate::tx_submission::tx_client_transport::tests::MockClientTransport;
    use amaru_mempool::strategies::InMemoryMempool;
    use pallas_network::miniprotocols::txsubmission::Message;
    use tokio::sync::mpsc;
    use tokio::sync::mpsc::{Receiver, Sender};
    use tokio::task::JoinHandle;

    #[tokio::test]
    async fn serve_transactions_blocking() -> anyhow::Result<()> {
        // Create a mempool with some transactions
        let mempool: Arc<InMemoryMempool<Tx>> = Arc::new(InMemoryMempool::default());
        let txs = vec![
            Tx::new("0d8d00cdd4657ac84d82f0a56067634a"),
            Tx::new("1d8d00cdd4657ac84d82f0a56067634a"),
            Tx::new("2d8d00cdd4657ac84d82f0a56067634a"),
            Tx::new("3d8d00cdd4657ac84d82f0a56067634a"),
            Tx::new("4d8d00cdd4657ac84d82f0a56067634a"),
        ];
        let era_tx_ids = txs
            .iter()
            .map(|tx| new_era_tx_id(tx.tx_id()))
            .collect::<Vec<_>>();
        let era_tx_bodies = txs
            .iter()
            .map(|tx| new_era_tx_body(tx.tx_body()))
            .collect::<Vec<_>>();
        for tx in txs.into_iter() {
            mempool.add(tx)?;
        }
        let (tx_req, mut replies, _client_handle) = start_client(mempool).await?;

        // Send requests to retrieve transactions and block until they are available.
        // In this case they are immediately available since we pre-populated the mempool.
        let requests = vec![
            Message::RequestTxIds(true, 0, 2),
            Message::RequestTxs(vec![era_tx_ids[0].clone(), era_tx_ids[1].clone()]),
            Message::RequestTxIds(true, 2, 2),
            Message::RequestTxs(vec![era_tx_ids[2].clone(), era_tx_ids[3].clone()]),
            Message::RequestTxIds(true, 2, 2),
            Message::RequestTxs(vec![era_tx_ids[4].clone()]),
        ];
        for r in requests {
            tx_req.send(r).await?;
        }

        // Check replies
        // We basically assert that we receive the expected ids and transactions
        // 2 by 2, then the last one, since we requested batches of 2.
        assert_next_message(&mut replies, Message::Init).await?;
        assert_tx_ids_reply(&mut replies, &era_tx_ids, &[0, 1]).await?;
        assert_tx_bodies_reply(&mut replies, &era_tx_bodies, &[0, 1]).await?;
        assert_tx_ids_reply(&mut replies, &era_tx_ids, &[2, 3]).await?;
        assert_tx_bodies_reply(&mut replies, &era_tx_bodies, &[2, 3]).await?;
        assert_tx_ids_reply(&mut replies, &era_tx_ids, &[4]).await?;
        assert_tx_bodies_reply(&mut replies, &era_tx_bodies, &[4]).await?;

        Ok(())
    }

    #[tokio::test]
    async fn serve_transactions_non_blocking() -> anyhow::Result<()> {
        // Create a mempool with some transactions
        let mempool: Arc<InMemoryMempool<Tx>> = Arc::new(InMemoryMempool::default());
        let txs = [
            Tx::new("0d8d00cdd4657ac84d82f0a56067634a"),
            Tx::new("1d8d00cdd4657ac84d82f0a56067634a"),
            Tx::new("2d8d00cdd4657ac84d82f0a56067634a"),
            Tx::new("3d8d00cdd4657ac84d82f0a56067634a"),
            Tx::new("4d8d00cdd4657ac84d82f0a56067634a"),
        ];
        let era_tx_ids = txs
            .iter()
            .map(|tx| new_era_tx_id(tx.tx_id()))
            .collect::<Vec<_>>();
        let era_tx_bodies = txs
            .iter()
            .map(|tx| new_era_tx_body(tx.tx_body()))
            .collect::<Vec<_>>();
        for tx in txs.iter().take(2) {
            mempool.add(tx.clone())?;
        }
        let (tx_req, mut replies, _client_handle) = start_client(mempool.clone()).await?;

        // Send requests to retrieve transactions and block until they are available.
        // In this case they are immediately available since we pre-populated the mempool.
        let requests = vec![
            Message::RequestTxIds(false, 0, 2),
            Message::RequestTxs(vec![era_tx_ids[0].clone(), era_tx_ids[1].clone()]),
            Message::RequestTxIds(false, 2, 2),
        ];
        for r in requests {
            tx_req.send(r).await?;
        }

        assert_next_message(&mut replies, Message::Init).await?;
        assert_tx_ids_reply(&mut replies, &era_tx_ids, &[0, 1]).await?;
        assert_tx_bodies_reply(&mut replies, &era_tx_bodies, &[0, 1]).await?;
        assert_tx_ids_reply(&mut replies, &era_tx_ids, &[]).await?;

        // Refill the mempool with more transactions
        for tx in &txs[2..] {
            mempool.add(tx.clone())?;
        }
        let requests = vec![
            Message::RequestTxIds(false, 2, 2),
            Message::RequestTxs(vec![era_tx_ids[2].clone(), era_tx_ids[3].clone()]),
            Message::RequestTxIds(false, 2, 2),
            Message::RequestTxs(vec![era_tx_ids[4].clone()]),
        ];
        for r in requests {
            tx_req.send(r).await?;
        }

        // Check replies
        // We basically assert that we receive the expected ids and transactions
        // 2 by 2, then the last one, since we requested batches of 2.
        assert_tx_ids_reply(&mut replies, &era_tx_ids, &[2, 3]).await?;
        assert_tx_bodies_reply(&mut replies, &era_tx_bodies, &[2, 3]).await?;
        assert_tx_ids_reply(&mut replies, &era_tx_ids, &[4]).await?;
        assert_tx_bodies_reply(&mut replies, &era_tx_bodies, &[4]).await?;

        Ok(())
    }

    // HELPERS

    /// Send a series of requests to a tx submission client backed by the given mempool.
    /// Returns a receiver to collect the messages that the client sent back.
    async fn start_client(
        mempool: Arc<dyn Mempool<Tx>>,
    ) -> anyhow::Result<(
        Sender<Message<EraTxId, EraTxBody>>,
        Receiver<Message<EraTxId, EraTxBody>>,
        JoinHandle<anyhow::Result<()>>,
    )> {
        let mut client = TxSubmissionClient::new(mempool.clone(), Peer::new("peer-1"));

        let (tx_req, rx_req) = mpsc::channel(10);
        let (tx_messages, rx_messages) = mpsc::channel(10);

        let transport = MockClientTransport::new(rx_req, tx_messages);

        let client_handle =
            tokio::spawn(async move { client.start_client_with_transport(transport).await });
        Ok((tx_req, rx_messages, client_handle))
    }
}
