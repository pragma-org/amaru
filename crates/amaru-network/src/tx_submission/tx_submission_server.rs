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
use amaru_kernel::Hash;
use amaru_kernel::peer::Peer;
use amaru_ouroboros_traits::{Mempool, TxId, TxOrigin};
use minicbor::{CborLen, Decode, Encode};
use pallas_network::miniprotocols::txsubmission::{
    Blocking, EraTxBody, EraTxId, Reply, TxIdAndSize,
};
use pallas_network::multiplexer::AgentChannel;
use std::cmp::Ordering;
use std::collections::{BTreeSet, VecDeque};
use std::sync::Arc;
use tracing::info;

/// Tx submission server state machine for a given peer.
///
/// The `window` field tracks the transactions that have been advertised to the peer.
/// The `last_seq` field tracks the last sequence number that has been acknowledged by the peer.
///
pub struct TxSubmissionServer<Tx: Send + Sync + 'static> {
    /// Mempool to pull transactions from.
    mempool: Arc<dyn Mempool<Tx>>,
    /// Server parameters: batch sizes, window sizes, etc.
    params: ServerParams,
    /// Peer we are connecting to.
    peer: Peer,
    /// All tx_ids advertised but not yet acked (and their size).
    window: VecDeque<(EraTxIdOrd, u32)>,
    /// Tx ids we want to fetch but haven't yet requested.
    pending_fetch: VecDeque<EraTxIdOrd>,
    /// Tx ids we requested; waiting for replies.
    /// First as a FIFO queue because when we receive tx bodies we don't get the ids back.
    inflight_fetch_queue: VecDeque<EraTxIdOrd>,
    /// Then as a set for quick lookup when processing received ids.
    /// This is kept in sync with `inflight_fetch_queue`. When we receive a tx body,
    /// we pop it from the front of the queue and remove it from the set.
    inflight_fetch_set: BTreeSet<EraTxIdOrd>,
    /// Tx ids we processed but didn't insert (invalid, policy failure, etc.).
    rejected: BTreeSet<EraTxIdOrd>,
}

#[derive(Debug, Clone)]
pub(crate) struct EraTxIdOrd(EraTxId);

impl PartialEq for EraTxIdOrd {
    fn eq(&self, other: &Self) -> bool {
        self.0.0 == other.0.0 && self.0.1 == other.0.1
    }
}

impl Eq for EraTxIdOrd {}

impl PartialOrd for EraTxIdOrd {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for EraTxIdOrd {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.0.0.cmp(&other.0.0) {
            Ordering::Equal => self.0.1.cmp(&other.0.1),
            other @ (Ordering::Less | Ordering::Greater) => other,
        }
    }
}

impl EraTxIdOrd {
    pub(crate) fn new(id: EraTxId) -> Self {
        Self(id)
    }
}

impl std::ops::Deref for EraTxIdOrd {
    type Target = EraTxId;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub struct ServerParams {
    max_window: usize,  // how many tx ids we keep in “window”
    fetch_batch: usize, // how many txs we request per round
    blocking: Blocking, // should the client block when we request more tx ids that it cannot serve
}

impl ServerParams {
    pub fn new(max_window: usize, fetch_batch: usize, blocking: Blocking) -> Self {
        Self {
            max_window,
            fetch_batch,
            blocking,
        }
    }
}

impl<Tx> TxSubmissionServer<Tx>
where
    Tx: for<'a> Decode<'a, ()> + Encode<()> + CborLen<()> + Send + Sync + 'static,
{
    /// Create a new tx submission server state machine for the given mempool and peer.
    pub fn new(params: ServerParams, mempool: Arc<dyn Mempool<Tx>>, peer: Peer) -> Self {
        Self {
            mempool: mempool.clone(),
            peer,
            params,
            window: VecDeque::new(),
            pending_fetch: VecDeque::new(),
            inflight_fetch_queue: VecDeque::new(),
            inflight_fetch_set: BTreeSet::new(),
            rejected: BTreeSet::new(),
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
        info!(peer = %self.peer,
            "Started tx submission server for peer {}",
            self.peer
        );
        transport.wait_for_init().await?;
        self.request_tx_ids(&mut transport).await?;
        loop {
            if transport.is_done().await? {
                break;
            }
            match transport.receive_next_reply().await? {
                Reply::TxIds(ids) => {
                    self.received_tx_ids(ids);
                    if let Some(tx_ids) = self.tx_ids_to_request() {
                        transport.request_txs(tx_ids).await?;
                    }
                }
                Reply::Txs(txs) => {
                    self.received_txs(txs)?;
                    self.request_tx_ids(&mut transport).await?;
                }
                Reply::Done => break,
            }
        }
        Ok(())
    }

    async fn request_tx_ids<T: TxServerTransport>(
        &mut self,
        transport: &mut T,
    ) -> anyhow::Result<()> {
        // Acknowledge everything we’ve already processed.
        let mut ack = 0_u16;

        while let Some((era_tx_id, _size)) = self.window.front() {
            let tx_id = TxId::new(Hash::from(era_tx_id.1.as_slice()));
            let already_in_mempool = self.mempool.contains(&tx_id);
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

        transport
            .acknowledge_and_request_tx_ids(self.params.blocking, ack, req)
            .await?;
        Ok(())
    }

    fn received_tx_ids(&mut self, txs: Vec<TxIdAndSize<EraTxId>>) {
        for tx_id_and_size in txs {
            let (era_tx_id, size) = (tx_id_and_size.0, tx_id_and_size.1);
            let tx_id = TxId::new(Hash::from(era_tx_id.1.as_slice()));
            let era_tx_id = EraTxIdOrd::new(era_tx_id);
            // We add the tx id to the window to acknowledge it on the next round.
            self.window.push_back((era_tx_id.clone(), size));

            // We only add to pending fetch if we haven't received it yet in the mempool.
            // and the tx id is not already rejected.
            if !self.mempool.contains(&tx_id) && !self.rejected.contains(&era_tx_id) {
                self.pending_fetch.push_back(era_tx_id);
            }
        }
    }

    fn tx_ids_to_request(&mut self) -> Option<Vec<EraTxId>> {
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

        if ids.is_empty() {
            None
        } else {
            Some(
                ids.into_iter()
                    .map(|era_tx_id_ord| era_tx_id_ord.0)
                    .collect(),
            )
        }
    }

    fn received_txs(&mut self, tx_bodies: Vec<EraTxBody>) -> anyhow::Result<()> {
        for tx_body in tx_bodies {
            // this is the exact id we requested for this body (FIFO)
            if let Some(requested_id) = self.inflight_fetch_queue.pop_front() {
                self.inflight_fetch_set.remove(&requested_id);

                let tx: Tx = minicbor::decode(tx_body.1.as_slice())?;
                let inserted = self.validate_tx(&tx)
                    && self
                    .mempool
                    .insert(tx, TxOrigin::Remote(self.peer.clone()))
                    .is_ok();
                if !inserted {
                    self.rejected.insert(requested_id);
                }
            }
        }
        Ok(())
    }

    fn validate_tx(&self, _tx: &Tx) -> bool {
        // Hook into amaru-ledger for local state validation.
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tx_submission::tests::{Tx, assert_next_message};
    use crate::tx_submission::tx_server_transport::tests::MockServerTransport;
    use crate::tx_submission::tx_submission_client::tests::*;
    use amaru_kernel::Hasher;
    use amaru_mempool::strategies::InMemoryMempool;
    use pallas_network::miniprotocols::txsubmission::Message;
    use tokio::sync::mpsc;
    use tokio::sync::mpsc::{Receiver, Sender};
    use tokio::task::JoinHandle;

    #[test]
    fn check_ids() {
        let tx = Tx::new("0d8d00cdd4657ac84d82f0a56067634a");
        let tx_id = tx.tx_id();
        let tx_body = tx.tx_body();
        assert_eq!(
            TxId::new(Hasher::<{ 32 * 8 }>::hash(tx_body.as_slice())),
            tx_id,
            "the tx id is the hash of the encoded body"
        );

        let era_tx_id = new_era_tx_id(tx_id.clone());
        let era_tx_body = new_era_tx_body(tx_body.clone());
        assert_eq!(
            TxId::new(Hash::from(era_tx_id.1.as_slice())),
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
        // Prepare some transactions to be used in the test
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
            Message::ReplyTxIds(vec![TxIdAndSize(era_tx_ids[4].clone(), 32)]),
            Message::ReplyTxs(vec![era_tx_bodies[4].clone()]),
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
            Message::RequestTxs(vec![era_tx_ids[4].clone()]),
        )
            .await?;
        Ok(())
    }

    // HELPERS

    /// Send a series of reply from a tx submission client backed by the given mempool.
    /// Returns a receiver to collect the messages that the server sent back.
    async fn start_server(
        mempool: Arc<dyn Mempool<Tx>>,
    ) -> anyhow::Result<(
        Sender<Message<EraTxId, EraTxBody>>,
        Receiver<Message<EraTxId, EraTxBody>>,
        JoinHandle<anyhow::Result<()>>,
    )> {
        let mut server = TxSubmissionServer::new(
            ServerParams::new(10, 2, true),
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
