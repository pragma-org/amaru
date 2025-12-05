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

use crate::tx_submission::tx_server_transport::{PallasTxServerTransport, TxServerTransport};
use crate::tx_submission::{ServerParams, tx_id_from_era_tx_id};
use amaru_kernel::peer::Peer;
use amaru_ouroboros_traits::{TxOrigin, TxSubmissionMempool};
use minicbor::{CborLen, Decode, Encode};
use pallas_network::miniprotocols::txsubmission::{EraTxBody, EraTxId, Reply, TxIdAndSize};
use pallas_network::multiplexer::AgentChannel;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, VecDeque};
use std::sync::Arc;
use tracing::info;

/// Tx submission server state machine for a given peer.
///
/// The `window` field tracks the transactions that have been advertised to the peer.
/// The `last_seq` field tracks the last sequence number that has been acknowledged by the peer.
///
pub struct TxSubmissionServer<Tx: Send + Sync + 'static> {
    /// Server parameters: batch sizes, window sizes, etc.
    params: ServerParams,
    /// Mempool to pull transactions from.
    mempool: Arc<dyn TxSubmissionMempool<Tx>>,
    /// State tracked for this peer.
    state: TxSubmissionServerState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxSubmissionServerState {
    /// Server parameters: batch sizes, window sizes, etc.
    params: ServerParams,
    /// Peer we are connecting to.
    peer: Peer,
    /// All tx_ids advertised but not yet acked (and their size).
    window: VecDeque<(EraTxId, u32)>,
    /// Tx ids we want to fetch but haven't yet requested.
    pending_fetch: VecDeque<EraTxId>,
    /// Tx ids we requested; waiting for replies.
    /// First as a FIFO queue because when we receive tx bodies we don't get the ids back.
    inflight_fetch_queue: VecDeque<EraTxId>,
    /// Then as a set for quick lookup when processing received ids.
    /// This is kept in sync with `inflight_fetch_queue`. When we receive a tx body,
    /// we pop it from the front of the queue and remove it from the set.
    inflight_fetch_set: BTreeSet<EraTxId>,
    /// Tx ids we processed but didn't insert (invalid, policy failure, etc.).
    rejected: BTreeSet<EraTxId>,
}

impl TxSubmissionServerState {
    pub fn new(peer: &Peer, params: ServerParams) -> Self {
        Self {
            params,
            peer: peer.clone(),
            window: VecDeque::new(),
            pending_fetch: VecDeque::new(),
            inflight_fetch_queue: VecDeque::new(),
            inflight_fetch_set: BTreeSet::new(),
            rejected: BTreeSet::new(),
        }
    }

    pub async fn process_tx_reply<Tx: Send + Sync + for<'a> Decode<'a, ()> + 'static>(
        &mut self,
        mempool: Arc<dyn TxSubmissionMempool<Tx>>,
        reply: Reply<EraTxId, EraTxBody>,
    ) -> anyhow::Result<TxResponse> {
        match reply {
            Reply::TxIds(tx_ids) => {
                self.received_tx_ids(mempool, tx_ids)?;
                Ok(TxResponse::NextTxs(self.txs_to_request()))
            }
            Reply::Txs(txs) => {
                self.received_txs(mempool.clone(), txs).await?;
                let (ack, req) = self.request_tx_ids(mempool.clone()).await?;
                Ok(TxResponse::NextTxIds(ack, req))
            }
            Reply::Done => Ok(TxResponse::Done),
        }
    }

    pub async fn request_tx_ids<Tx: Send + Sync + 'static>(
        &mut self,
        mempool: Arc<dyn TxSubmissionMempool<Tx>>,
    ) -> anyhow::Result<(u16, u16)> {
        // Acknowledge everything we’ve already processed.
        let mut ack = 0_u16;

        while let Some((era_tx_id, _size)) = self.window.front() {
            let tx_id = tx_id_from_era_tx_id(era_tx_id);
            let already_in_mempool = mempool.contains(&tx_id);
            let already_rejected = self.rejected.contains(era_tx_id);

            if already_in_mempool || already_rejected {
                // pop from window and ack it
                if let Some((front_id, _)) = self.window.pop_front() {
                    // keep rejected set from growing forever
                    if already_rejected {
                        self.rejected.remove(&front_id);
                    }
                    ack = ack.saturating_add(1);
                }
            } else {
                break;
            }
        }

        // Request as many as we can fit in the window.
        // Note: we cap at u16::MAX because the protocol uses u16 for counts.
        let req = self
            .params
            .max_window
            .saturating_sub(self.window.len())
            .min(u16::MAX as usize) as u16;
        Ok((ack, req))
    }

    pub fn received_tx_ids<Tx: Send + Sync + 'static>(
        &mut self,
        mempool: Arc<dyn TxSubmissionMempool<Tx>>,
        tx_ids: Vec<TxIdAndSize<EraTxId>>,
    ) -> anyhow::Result<()> {
        if self.params.blocking.into() && tx_ids.len() > self.params.max_window {
            return Err(anyhow::anyhow!(
                "Too transactions ids received in blocking mode"
            ));
        }

        for tx_id_and_size in tx_ids {
            let (era_tx_id, size) = (tx_id_and_size.0, tx_id_and_size.1);
            let tx_id = tx_id_from_era_tx_id(&era_tx_id);
            // We add the tx id to the window to acknowledge it on the next round.
            self.window.push_back((era_tx_id.clone(), size));

            // We only add to pending fetch if we haven't received it yet in the mempool.
            // and the tx id is not already rejected.
            if !mempool.contains(&tx_id) && !self.rejected.contains(&era_tx_id) {
                self.pending_fetch.push_back(era_tx_id);
            }
        }

        Ok(())
    }

    pub fn txs_to_request(&mut self) -> Option<Vec<EraTxId>> {
        let mut ids = Vec::new();

        while ids.len() < self.params.fetch_batch {
            if let Some(id) = self.pending_fetch.pop_front() {
                self.inflight_fetch_queue.push_back(id.clone());
                self.inflight_fetch_set.insert(id.clone());
                ids.push(id);
            } else {
                break;
            }
        }

        if ids.is_empty() { None } else { Some(ids) }
    }

    pub async fn received_txs<Tx: Send + Sync + for<'a> Decode<'a, ()> + 'static>(
        &mut self,
        mempool: Arc<dyn TxSubmissionMempool<Tx>>,
        tx_bodies: Vec<EraTxBody>,
    ) -> anyhow::Result<()> {
        if self.params.blocking.into() && tx_bodies.len() > self.params.fetch_batch {
            return Err(anyhow::anyhow!(
                "Too many transactions received in blocking mode"
            ));
        }

        for tx_body in tx_bodies {
            // this is the exact id we requested for this body (FIFO)
            if let Some(requested_id) = self.inflight_fetch_queue.pop_front() {
                self.inflight_fetch_set.remove(&requested_id);

                let tx: Tx = minicbor::decode(tx_body.1.as_slice())?;
                let inserted = mempool.validate_transaction(&tx).is_ok()
                    && mempool
                        .insert(tx, TxOrigin::Remote(self.peer.clone()))
                        .is_ok();
                if !inserted {
                    self.rejected.insert(requested_id);
                }
            }
        }
        Ok(())
    }
}

pub enum TxResponse {
    Done,
    NextTxIds(u16, u16),
    NextTxs(Option<Vec<EraTxId>>),
}

impl<Tx> TxSubmissionServer<Tx>
where
    Tx: for<'a> Decode<'a, ()> + Encode<()> + CborLen<()> + Send + Sync + 'static,
{
    /// Create a new tx submission server state machine for the given mempool and peer.
    pub fn new(
        params: ServerParams,
        mempool: Arc<dyn TxSubmissionMempool<Tx>>,
        peer: Peer,
    ) -> Self {
        let state = TxSubmissionServerState::new(&peer, params.clone());
        Self {
            mempool: mempool.clone(),
            params,
            state,
        }
    }

    /// Start the tx submission server state machine over the given agent channel.
    /// This function drives the state machine until completion.
    pub async fn start_server(&mut self, agent_channel: AgentChannel) -> anyhow::Result<()> {
        let transport = PallasTxServerTransport::new(agent_channel);
        self.start_server_with_transport(transport).await
    }

    /// Core server state machine, parameterized over a transport for testability.
    pub async fn start_server_with_transport<T: TxServerTransport>(
        &mut self,
        mut transport: T,
    ) -> anyhow::Result<()> {
        info!(peer = %self.state.peer,
            "Started tx submission server for peer {}",
            self.state.peer
        );
        transport.wait_for_init().await?;
        let (ack, req) = self.state.request_tx_ids(self.mempool.clone()).await?;
        transport
            .acknowledge_and_request_tx_ids(self.params.blocking, ack, req)
            .await?;
        loop {
            if transport.is_done().await? {
                break;
            }
            let tx_reply = transport.receive_next_reply().await?;
            let response = self
                .state
                .process_tx_reply(self.mempool.clone(), tx_reply)
                .await?;
            match response {
                TxResponse::NextTxIds(ack, req) => {
                    transport
                        .acknowledge_and_request_tx_ids(self.params.blocking, ack, req)
                        .await?;
                }
                TxResponse::NextTxs(tx_ids) => {
                    if let Some(tx_ids) = tx_ids {
                        transport.request_txs(tx_ids).await?;
                    }
                }
                TxResponse::Done => {
                    break;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tx_submission::conversions::{new_era_tx_id, tx_id_from_era_tx_id};
    use crate::tx_submission::tests::{
        Tx, assert_next_message, create_transactions, to_era_tx_bodies, to_era_tx_ids,
    };
    use crate::tx_submission::tx_server_transport::tests::MockServerTransport;
    use crate::tx_submission::{Blocking, new_era_tx_body_from_vec};
    use amaru_kernel::tx_submission_events::TxId;
    use amaru_kernel::{Hasher, to_cbor};
    use amaru_mempool::strategies::InMemoryMempool;
    use pallas_network::miniprotocols::txsubmission::Message;
    use tokio::sync::mpsc;
    use tokio::sync::mpsc::{Receiver, Sender};
    use tokio::task::JoinHandle;

    #[test]
    fn check_ids() {
        let tx = Tx::new("0d8d00cdd4657ac84d82f0a56067634a");
        let tx_id = tx.tx_id();
        let tx_body = to_cbor(&tx);
        assert_eq!(
            TxId::new(Hasher::<{ 32 * 8 }>::hash(tx_body.as_slice())),
            tx_id,
            "the tx id is the hash of the encoded body"
        );

        let era_tx_id = new_era_tx_id(tx_id.clone());
        let era_tx_body = new_era_tx_body_from_vec(tx_body.clone());
        assert_eq!(
            tx_id_from_era_tx_id(&era_tx_id),
            tx_id,
            "the era tx id transports the tx id hash"
        );
        assert_eq!(
            era_tx_body.1,
            tx_body.clone(),
            "sanity check: the era tx body transports the tx body"
        );
        assert_eq!(
            TxId::new(Hasher::<{ 32 * 8 }>::hash(era_tx_body.1.as_slice())),
            tx_id,
            "hashing the era tx body yields the same tx id"
        );
    }

    #[tokio::test]
    async fn test_server() -> anyhow::Result<()> {
        let txs = create_transactions(6);
        let era_tx_ids = to_era_tx_ids(&txs);
        let era_tx_bodies = to_era_tx_bodies(&txs);

        // Create a mempool with no initial transactions
        // since we are going to fetch them from a client
        let mempool: Arc<InMemoryMempool<Tx>> = Arc::new(InMemoryMempool::default());
        let (tx_reply, mut messages, _server_handle) = start_server(mempool).await?;

        // Send replies from the client as if they were replies to previous requests from the server
        let replies = vec![
            Message::Init,
            Message::ReplyTxIds(vec![
                TxIdAndSize(era_tx_ids[0].clone(), 32),
                TxIdAndSize(era_tx_ids[1].clone(), 32),
            ]),
            Message::ReplyTxs(vec![era_tx_bodies[0].clone(), era_tx_bodies[1].clone()]),
            Message::ReplyTxIds(vec![
                TxIdAndSize(era_tx_ids[2].clone(), 32),
                TxIdAndSize(era_tx_ids[3].clone(), 32),
            ]),
            Message::ReplyTxs(vec![era_tx_bodies[2].clone(), era_tx_bodies[3].clone()]),
            Message::ReplyTxIds(vec![
                TxIdAndSize(era_tx_ids[4].clone(), 32),
                TxIdAndSize(era_tx_ids[5].clone(), 32),
            ]),
            Message::ReplyTxs(vec![era_tx_bodies[4].clone(), era_tx_bodies[5].clone()]),
            Message::Done,
        ];
        for r in replies {
            tx_reply.send(r).await?;
        }

        // Check messages sent by the server
        assert_next_message(&mut messages, Message::RequestTxIds(true, 0, 10)).await?;
        assert_next_message(
            &mut messages,
            Message::RequestTxs(vec![era_tx_ids[0].clone(), era_tx_ids[1].clone()]),
        )
        .await?;
        assert_next_message(&mut messages, Message::RequestTxIds(true, 2, 10)).await?;
        assert_next_message(
            &mut messages,
            Message::RequestTxs(vec![era_tx_ids[2].clone(), era_tx_ids[3].clone()]),
        )
        .await?;
        assert_next_message(&mut messages, Message::RequestTxIds(true, 2, 10)).await?;
        assert_next_message(
            &mut messages,
            Message::RequestTxs(vec![era_tx_ids[4].clone(), era_tx_ids[5].clone()]),
        )
        .await?;
        Ok(())
    }

    #[tokio::test]
    async fn in_blocking_mode_the_returned_tx_ids_should_respect_the_batch_size()
    -> anyhow::Result<()> {
        let txs = create_transactions(4);
        let era_tx_ids = to_era_tx_ids(&txs);

        // Create a mempool with no initial transactions
        // since we are going to fetch them from a client
        let mempool: Arc<InMemoryMempool<Tx>> = Arc::new(InMemoryMempool::default());
        let (tx_reply, mut messages, _server_handle) = start_server(mempool).await?;

        let replies = vec![
            Message::Init,
            Message::ReplyTxIds(vec![TxIdAndSize(era_tx_ids[0].clone(), 32)]),
        ];
        for r in replies {
            tx_reply.send(r).await?;
        }

        // Check messages sent by the server
        assert_next_message(&mut messages, Message::RequestTxIds(true, 0, 10)).await?;
        Ok(())
    }

    // HELPERS

    /// Send a series of reply from a tx submission client backed by the given mempool.
    /// Returns a receiver to collect the messages that the server sent back.
    async fn start_server(
        mempool: Arc<dyn TxSubmissionMempool<Tx>>,
    ) -> anyhow::Result<(
        Sender<Message<EraTxId, EraTxBody>>,
        Receiver<Message<EraTxId, EraTxBody>>,
        JoinHandle<anyhow::Result<()>>,
    )> {
        let mut server = TxSubmissionServer::new(
            ServerParams::new(10, 2, Blocking::Yes),
            mempool.clone(),
            Peer::new("peer-1"),
        );

        let (tx_reply, rx_reply) = mpsc::channel(10);
        let (tx_message, rx_message) = mpsc::channel(10);

        let transport = MockServerTransport::new(rx_reply, tx_message);

        let server_handle =
            tokio::spawn(async move { server.start_server_with_transport(transport).await });
        Ok((tx_reply, rx_message, server_handle))
    }
}
