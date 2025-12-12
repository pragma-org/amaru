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

use crate::consensus::tx_submission::tests::TxServerTransport;
use crate::consensus::tx_submission::{Blocking, ServerParams, TxSubmissionServerState};
use amaru_kernel::peer::Peer;
use amaru_network::{era_tx_id, from_pallas_reply};
use amaru_ouroboros_traits::{TxClientReply, TxSubmissionMempool};
use pallas_network::miniprotocols::txsubmission::{EraTxBody, EraTxId};
use pallas_primitives::conway::Tx;
use std::sync::Arc;
use tracing::info;

/// Tx submission server state machine for a given peer.
///
/// Most of the logic is in the `TxSubmissionServerState` struct, this struct just
/// ties it together with a mempool and a transport.
///
pub struct TxSubmissionServer<Tx: Send + Sync + 'static> {
    /// Mempool to pull transactions from.
    mempool: Arc<dyn TxSubmissionMempool<Tx>>,
    /// State tracked for this peer.
    state: TxSubmissionServerState,
}

impl TxSubmissionServer<Tx> {
    /// Create a new tx submission server state machine for the given mempool and peer.
    pub fn new(
        params: ServerParams,
        mempool: Arc<dyn TxSubmissionMempool<Tx>>,
        peer: Peer,
    ) -> Self {
        let state = TxSubmissionServerState::new(&peer, params.clone());
        Self {
            mempool: mempool.clone(),
            state,
        }
    }

    /// Core server state machine, parameterized over a transport for testability.
    pub async fn start_server_with_transport<T: TxServerTransport + Send + 'static>(
        mut self,
        mut transport: T,
    ) -> anyhow::Result<()> {
        info!(peer = %self.state.peer(),
            "Started tx submission server for peer {}",
            self.state.peer()
        );
        transport.wait_for_init().await?;
        let (ack, req, _blocking) = self.state.request_tx_ids(self.mempool.clone()).await?;
        // The first request is always blocking
        transport
            .acknowledge_and_request_tx_ids(ack, req, Blocking::Yes)
            .await?;
        loop {
            if transport.is_done().await? {
                break;
            }
            let tx_reply = transport.receive_next_reply().await?;
            match from_pallas_reply(self.state.peer(), tx_reply)? {
                TxClientReply::Done { .. } => {}
                TxClientReply::Init { .. } => {}
                TxClientReply::TxIds { tx_ids, .. } => {
                    if let Some(txs_to_request) = self
                        .state
                        .process_tx_ids_reply(self.mempool.clone(), tx_ids)?
                    {
                        transport
                            .request_txs(txs_to_request.into_iter().map(era_tx_id).collect())
                            .await?;
                    }
                }
                TxClientReply::Txs { txs, .. } => {
                    let (ack, req, blocking) = self
                        .state
                        .process_txs_reply(self.mempool.clone(), txs)
                        .await?;
                    transport
                        .acknowledge_and_request_tx_ids(ack, req, blocking)
                        .await?;
                }
            }
        }
        Ok(())
    }
}

mod tests {
    use super::*;
    use crate::consensus::tx_submission::tests::{
        MockServerTransport, assert_next_message, create_transaction, create_transactions,
    };
    use amaru_kernel::{Hasher, to_cbor};
    use amaru_mempool::strategies::InMemoryMempool;
    use amaru_network::{era_tx_bodies, era_tx_body_from_vec, era_tx_ids, tx_id_from_era_tx_id};
    use amaru_ouroboros_traits::TxId;
    use pallas_network::miniprotocols::txsubmission::{Message, TxIdAndSize};
    use pallas_primitives::conway::Tx;
    use tokio::sync::mpsc;
    use tokio::sync::mpsc::{Receiver, Sender};
    use tokio::task::JoinHandle;

    #[test]
    fn check_ids() {
        let tx = create_transaction(0);
        let tx_id = TxId::from(&tx);
        let tx_body = to_cbor(&tx);
        assert_eq!(
            TxId::new(Hasher::<{ 32 * 8 }>::hash(tx_body.as_slice())),
            tx_id,
            "the tx id is the hash of the encoded body"
        );

        let era_tx_id = era_tx_id(tx_id);
        let era_tx_body = era_tx_body_from_vec(tx_body.clone());
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
        let mut tx_ids = vec![];
        for tx in &txs {
            tx_ids.push(TxId::from(tx));
        }
        let era_tx_ids = era_tx_ids(&tx_ids);
        let era_tx_bodies = era_tx_bodies(&txs);

        // Create a mempool with no initial transactions
        // since we are going to fetch them from a client
        let mempool: Arc<InMemoryMempool<Tx>> = Arc::new(InMemoryMempool::default());
        let (tx_reply, mut messages, _server_handle) = start_server(mempool).await?;

        // Send replies from the client as if they were replies to previous requests from the server
        let replies = vec![
            Message::Init,
            Message::ReplyTxIds(vec![
                TxIdAndSize(era_tx_ids[0].clone(), 50),
                TxIdAndSize(era_tx_ids[1].clone(), 50),
            ]),
            Message::ReplyTxs(vec![era_tx_bodies[0].clone(), era_tx_bodies[1].clone()]),
            Message::ReplyTxIds(vec![
                TxIdAndSize(era_tx_ids[2].clone(), 50),
                TxIdAndSize(era_tx_ids[3].clone(), 50),
            ]),
            Message::ReplyTxs(vec![era_tx_bodies[2].clone(), era_tx_bodies[3].clone()]),
            Message::ReplyTxIds(vec![
                TxIdAndSize(era_tx_ids[4].clone(), 50),
                TxIdAndSize(era_tx_ids[5].clone(), 50),
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
        let mut tx_ids = vec![];
        for tx in &txs {
            tx_ids.push(TxId::from(tx));
        }
        let era_tx_ids = era_tx_ids(&tx_ids);

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
        let server = TxSubmissionServer::new(
            ServerParams::new(10, 2),
            mempool.clone(),
            Peer::new("peer-1"),
        );

        let (tx_reply, rx_reply) = mpsc::channel(10);
        let (tx_message, rx_message) = mpsc::channel(10);

        let transport = MockServerTransport::new(rx_reply, tx_message);

        let server_handle = tokio::spawn(server.start_server_with_transport(transport));
        Ok((tx_reply, rx_message, server_handle))
    }
}
