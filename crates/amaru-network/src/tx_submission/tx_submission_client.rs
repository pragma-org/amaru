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
use crate::tx_submission::tx_id_from_era_tx_id;
use amaru_kernel::peer::Peer;
use amaru_kernel::to_cbor;
use amaru_kernel::tx_submission_events::TxId;
use amaru_ouroboros_traits::{MempoolSeqNo, TxSubmissionMempool};
use minicbor::Encode;
use pallas_network::miniprotocols::txsubmission;
use pallas_network::miniprotocols::txsubmission::{EraTxBody, EraTxId, Request, TxIdAndSize};
use pallas_traverse::Era;
use pure_stage::typetag::__private21::serde::Serialize;
use serde::Deserialize;
use std::collections::VecDeque;
use std::fmt::Debug;
use std::sync::Arc;
use tracing::{debug, info};

/// Tx submission client state machine for a given peer.
///
/// The `window` field tracks the transactions that have been advertised to the peer.
/// The `last_seq` field tracks the last sequence number that has been acknowledged by the peer.
///
#[derive(Clone)]
pub struct TxSubmissionClient<Tx> {
    /// Mempool to pull transactions from.
    mempool: Arc<dyn TxSubmissionMempool<Tx>>,
    /// In-memory state of the client.
    state: TxSubmissionClientState,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxSubmissionClientState {
    /// Peer we are serving.
    peer: Peer,
    /// What we’ve already advertised but has not yet been fully acked.
    window: VecDeque<(TxId, MempoolSeqNo)>,
    /// Last seq_no we have ever pulled from the mempool for this peer.
    /// None if we have not pulled anything yet.
    last_seq: Option<MempoolSeqNo>,
}

impl TxSubmissionClientState {
    pub fn new(peer: &Peer) -> Self {
        Self {
            peer: peer.clone(),
            window: VecDeque::new(),
            last_seq: None,
        }
    }

    pub async fn process_tx_request<Tx: Send + Debug + Sync + 'static>(
        &mut self,
        mempool: Arc<dyn TxSubmissionMempool<Tx>>,
        request: Request<EraTxId>,
    ) -> anyhow::Result<TxResponse<Tx>> {
        match request {
            Request::TxIds(acknowledged, required_next) => {
                if required_next == 0 {
                    debug!(peer = %self.peer,
                        "Requested 0 tx ids, terminating tx submission client",
                    );
                    return Ok(TxResponse::<Tx>::Done);
                }
                if !mempool
                    .wait_for_at_least(mempool.last_seq_no().add(required_next as u64))
                    .await
                {
                    return Ok(TxResponse::<Tx>::Done);
                }
                let tx_ids = self
                    .get_next_tx_ids(mempool, acknowledged, required_next)
                    .await?;
                Ok(TxResponse::NextIds(tx_ids))
            }
            Request::TxIdsNonBlocking(acknowledged, required_next) => Ok(TxResponse::NextIds(
                self.get_next_tx_ids(mempool, acknowledged, required_next)
                    .await?,
            )),
            Request::Txs(ids) => {
                if ids.is_empty() {
                    debug!(peer = %self.peer,
                        "Requested 0 txs, terminating tx submission client"
                    );
                    return Ok(TxResponse::<Tx>::Done);
                }
                let ids: Vec<TxId> = ids.iter().map(tx_id_from_era_tx_id).collect();
                if ids
                    .iter()
                    .any(|id| !self.window.iter().any(|(wid, _)| wid == id))
                {
                    debug!(peer = %self.peer,
                        "Requested unknown tx ids, terminating tx submission client"
                    );
                    return Ok(TxResponse::<Tx>::Done);
                }
                let txs = mempool.get_txs_for_ids(ids.as_slice());
                if txs.is_empty() {
                    Ok(TxResponse::<Tx>::Done)
                } else {
                    Ok(TxResponse::NextTxs(txs))
                }
            }
        }
    }

    /// Take notice of the acknowledged transactions, and send the next batch of tx ids.
    async fn get_next_tx_ids<Tx: Send + Debug + Sync + 'static>(
        &mut self,
        mempool: Arc<dyn TxSubmissionMempool<Tx>>,
        acknowledged: u16,
        required_next: u16,
    ) -> anyhow::Result<Vec<(TxId, u32)>> {
        self.discard(acknowledged);
        let tx_ids = mempool.tx_ids_since(self.next_seq(), required_next);
        let result = tx_ids
            .clone()
            .into_iter()
            .map(|(tx_id, tx_size, _)| (tx_id, tx_size))
            .collect();
        self.update(tx_ids);
        Ok(result)
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

impl<Tx: Encode<()> + Send + Debug + Sync + 'static> TxSubmissionClient<Tx> {
    /// Create a new tx submission client state machine for the given mempool and peer.
    pub fn new(mempool: Arc<dyn TxSubmissionMempool<Tx>>, peer: &Peer) -> Self {
        Self {
            mempool: mempool.clone(),
            state: TxSubmissionClientState::new(peer),
        }
    }

    /// Start the tx submission client state machine over the given agent channel.
    /// This function drives the state machine until completion.
    pub async fn start_client(&mut self, client: txsubmission::Client) -> anyhow::Result<()> {
        let transport = PallasTxClientTransport::new(client);
        self.start_client_with_transport(transport).await
    }

    /// Core state machine, parameterized over a transport for testability.
    pub async fn start_client_with_transport<T: TxClientTransport>(
        &mut self,
        mut transport: T,
    ) -> anyhow::Result<()> {
        transport.send_init().await?;
        info!(peer = %self.state.peer,
            "Started tx submission client"
        );
        loop {
            let request = match transport.next_request().await {
                Ok(r) => r,
                Err(e) => {
                    debug!(peer = %self.state.peer,
                        "Error receiving next request, terminating tx submission client: {e:?}"
                    );
                    break;
                }
            };
            match self
                .state
                .process_tx_request(self.mempool.clone(), request)
                .await?
            {
                TxResponse::Done => {
                    transport.send_done().await?;
                    break;
                }
                TxResponse::NextIds(tx_ids) => {
                    transport
                        .reply_tx_ids(
                            tx_ids
                                .iter()
                                .map(|(tx_id, tx_size)| {
                                    TxIdAndSize(
                                        EraTxId(Era::Conway.into(), tx_id.to_vec()),
                                        *tx_size,
                                    )
                                })
                                .collect(),
                        )
                        .await?
                }
                TxResponse::NextTxs(txs) => {
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
            };
        }
        Ok(())
    }
}

pub enum TxResponse<Tx> {
    Done,
    NextIds(Vec<(TxId, u32)>),
    NextTxs(Vec<Arc<Tx>>),
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::tx_submission::tests::{
        SizedMempool, Tx, assert_next_message, assert_tx_bodies_reply, assert_tx_ids_reply,
        create_transactions, to_era_tx_bodies, to_era_tx_ids,
    };
    use crate::tx_submission::tx_client_transport::tests::MockClientTransport;
    use amaru_ouroboros_traits::Mempool;
    use pallas_network::miniprotocols::txsubmission::Message;
    use tokio::sync::mpsc;
    use tokio::sync::mpsc::{Receiver, Sender};
    use tokio::task::JoinHandle;

    #[tokio::test]
    async fn serve_transactions_blocking() -> anyhow::Result<()> {
        // Create a mempool with some transactions
        let mempool = Arc::new(SizedMempool::with_capacity(6));
        let txs = create_transactions(6);
        let era_tx_ids = to_era_tx_ids(&txs);
        let era_tx_bodies = to_era_tx_bodies(&txs);
        for tx in txs.into_iter() {
            mempool.add(tx)?;
        }
        let (tx_req, mut replies, _rx_observe_messages, _client_handle) =
            start_client(mempool).await?;

        // Send requests to retrieve transactions and block until they are available.
        // In this case they are immediately available since we pre-populated the mempool.
        let requests = vec![
            Message::RequestTxIds(true, 0, 2),
            Message::RequestTxs(vec![era_tx_ids[0].clone(), era_tx_ids[1].clone()]),
            Message::RequestTxIds(true, 2, 2),
            Message::RequestTxs(vec![era_tx_ids[2].clone(), era_tx_ids[3].clone()]),
            Message::RequestTxIds(true, 2, 2),
            Message::RequestTxs(vec![era_tx_ids[4].clone(), era_tx_ids[5].clone()]),
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
        assert_tx_ids_reply(&mut replies, &era_tx_ids, &[4, 5]).await?;
        assert_tx_bodies_reply(&mut replies, &era_tx_bodies, &[4, 5]).await?;

        Ok(())
    }

    #[tokio::test]
    async fn serve_transactions_non_blocking() -> anyhow::Result<()> {
        // Create a mempool with some transactions
        let mempool = Arc::new(SizedMempool::with_capacity(6));
        let txs = create_transactions(6);
        let era_tx_ids = to_era_tx_ids(&txs);
        let era_tx_bodies = to_era_tx_bodies(&txs);
        for tx in txs.iter().take(2) {
            mempool.add(tx.clone())?;
        }
        let (tx_req, mut replies, _rx_observe_messages, _client_handle) =
            start_client(mempool.clone()).await?;

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

        // Refill the mempool with more transactions
        for tx in &txs[2..] {
            mempool.add(tx.clone())?;
        }
        let requests = vec![
            Message::RequestTxIds(false, 2, 2),
            Message::RequestTxs(vec![era_tx_ids[2].clone(), era_tx_ids[3].clone()]),
            Message::RequestTxIds(false, 2, 2),
            Message::RequestTxs(vec![era_tx_ids[4].clone(), era_tx_ids[5].clone()]),
        ];
        for r in requests {
            tx_req.send(r).await?;
        }

        // Check replies
        // We basically assert that we receive the expected ids and transactions
        // 2 by 2, then the last one, since we requested batches of 2.
        assert_tx_ids_reply(&mut replies, &era_tx_ids, &[]).await?;
        assert_tx_ids_reply(&mut replies, &era_tx_ids, &[2, 3]).await?;
        assert_tx_bodies_reply(&mut replies, &era_tx_bodies, &[2, 3]).await?;
        assert_tx_ids_reply(&mut replies, &era_tx_ids, &[4, 5]).await?;
        assert_tx_bodies_reply(&mut replies, &era_tx_bodies, &[4, 5]).await?;

        Ok(())
    }

    #[tokio::test]
    async fn request_txs_must_come_from_requested_ids() -> anyhow::Result<()> {
        // Create a mempool with some transactions
        let mempool = Arc::new(SizedMempool::with_capacity(6));
        let txs = create_transactions(4);
        let era_tx_ids = to_era_tx_ids(&txs);
        for tx in txs.iter() {
            mempool.add(tx.clone())?;
        }
        let (tx_req, mut replies, _rx_observe_messages, _client_handle) =
            start_client(mempool.clone()).await?;

        // Send requests to retrieve transactions and block until they are available.
        // In this case they are immediately available since we pre-populated the mempool.
        // The reply to the first request will be tx ids 0 and 1, which means that the server
        // should then request transactions for those ids only.
        // In this test we receive a request for tx ids 2 and 3, which were not advertised yet,
        // so the client should terminate the session.
        let requests = vec![
            Message::RequestTxIds(true, 0, 2),
            Message::RequestTxs(vec![era_tx_ids[2].clone(), era_tx_ids[3].clone()]),
        ];
        for r in requests {
            tx_req.send(r).await?;
        }

        assert_next_message(&mut replies, Message::Init).await?;
        assert_tx_ids_reply(&mut replies, &era_tx_ids, &[0, 1]).await?;
        assert_next_message(&mut replies, Message::Done).await?;
        Ok(())
    }

    #[tokio::test]
    async fn requested_ids_must_be_greater_than_0() -> anyhow::Result<()> {
        let mempool = Arc::new(SizedMempool::with_capacity(6));
        let (tx_req, mut replies, _rx_observe_messages, _client_handle) =
            start_client(mempool.clone()).await?;

        let requests = vec![Message::RequestTxIds(true, 0, 0)];
        for r in requests {
            tx_req.send(r).await?;
        }

        assert_next_message(&mut replies, Message::Init).await?;
        assert_next_message(&mut replies, Message::Done).await?;
        Ok(())
    }

    #[tokio::test]
    async fn requested_txs_must_be_greater_than_0() -> anyhow::Result<()> {
        let mempool = Arc::new(SizedMempool::with_capacity(4));
        let txs = create_transactions(4);
        let era_tx_ids = to_era_tx_ids(&txs);
        for tx in txs.iter() {
            mempool.add(tx.clone())?;
        }
        let (tx_req, mut replies, _rx_observe_messages, _client_handle) =
            start_client(mempool.clone()).await?;

        let requests = vec![
            Message::RequestTxIds(true, 0, 2),
            Message::RequestTxs(vec![]),
        ];
        for r in requests {
            tx_req.send(r).await?;
        }

        assert_next_message(&mut replies, Message::Init).await?;
        assert_tx_ids_reply(&mut replies, &era_tx_ids, &[0, 1]).await?;
        assert_next_message(&mut replies, Message::Done).await?;
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
        Receiver<Message<EraTxId, EraTxBody>>,
        JoinHandle<anyhow::Result<()>>,
    )> {
        let mut client = TxSubmissionClient::new(mempool.clone(), &Peer::new("peer-1"));

        let (tx_req, rx_req) = mpsc::channel(10);
        let (tx_messages, rx_messages) = mpsc::channel(10);
        let (tx_observe_messages, rx_observe_messages) = mpsc::channel(10);

        let transport = MockClientTransport::new(rx_req, tx_observe_messages, tx_messages);

        let client_handle =
            tokio::spawn(async move { client.start_client_with_transport(transport).await });
        Ok((tx_req, rx_messages, rx_observe_messages, client_handle))
    }
}
