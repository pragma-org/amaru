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

use amaru_kernel::peer::Peer;
use amaru_kernel::{Hash, to_cbor};
use amaru_ouroboros_traits::{Mempool, MempoolSeqNo, TxId};
use async_trait::async_trait;
use minicbor::{CborLen, Encode};
use pallas_network::miniprotocols::txsubmission;
use pallas_network::miniprotocols::txsubmission::{EraTxBody, EraTxId, Request, TxIdAndSize};
use pallas_network::multiplexer::AgentChannel;
use pallas_traverse::Era;
use std::collections::VecDeque;
use std::sync::Arc;

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
    async fn start_client_with_transport<T: TxClientTransport>(
        &mut self,
        mut transport: T,
    ) -> anyhow::Result<()> {
        transport.send_init().await?;
        loop {
            let request = match transport.next_request().await {
                Ok(r) => r,
                Err(_) => {
                    transport.send_done().await.ok();
                    break;
                }
            };
            match request {
                Request::TxIds(acknowledged, required_next) => {
                    if !self.mempool.wait_for_at_least(required_next).await {
                        transport.send_done().await?;
                        break;
                    }
                    self.provide_transactions(&mut transport, acknowledged, required_next)
                        .await?;
                }
                Request::TxIdsNonBlocking(acknowledged, required_next) => {
                    self.provide_transactions(&mut transport, acknowledged, required_next)
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

    async fn provide_transactions<T: TxClientTransport>(
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
    use super::*;
    use amaru_kernel::cbor::Encode;
    use amaru_mempool::strategies::InMemoryMempool;
    use minicbor::{Decode, Decoder, Encoder};
    use minicbor::encode::{Error, Write};
    use pallas_network::miniprotocols::txsubmission::Message;
    use pallas_network::miniprotocols::txsubmission::Message::{ReplyTxIds, ReplyTxs};
    use std::fmt::{Debug, Display};

    use crate::tx_submission::tx_submission_server::EraTxIdOrd;
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
            Request::TxIds(0, 2),
            Request::Txs(vec![era_tx_ids[0].clone(), era_tx_ids[1].clone()]),
            Request::TxIds(2, 2),
            Request::Txs(vec![era_tx_ids[2].clone(), era_tx_ids[3].clone()]),
            Request::TxIds(2, 2),
            Request::Txs(vec![era_tx_ids[4].clone()]),
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
        for tx in txs.iter().take(2) {
            mempool.add(tx.clone())?;
        }
        let (tx_req, mut replies, _client_handle) = start_client(mempool.clone()).await?;

        // Send requests to retrieve transactions and block until they are available.
        // In this case they are immediately available since we pre-populated the mempool.
        let requests = vec![
            Request::TxIdsNonBlocking(0, 2),
            Request::Txs(vec![era_tx_ids[0].clone(), era_tx_ids[1].clone()]),
            Request::TxIdsNonBlocking(2, 2),
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
            Request::TxIdsNonBlocking(2, 2),
            Request::Txs(vec![era_tx_ids[2].clone(), era_tx_ids[3].clone()]),
            Request::TxIdsNonBlocking(2, 2),
            Request::Txs(vec![era_tx_ids[4].clone()]),
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

    /// Check that the next message is a ReplyTxIds with the expected ids.
    pub(crate) async fn assert_tx_ids_reply(
        rx_messages: &mut Receiver<Message<EraTxId, EraTxBody>>,
        era_tx_ids: &[EraTxId],
        expected_ids: &[usize],
    ) -> anyhow::Result<()> {
        let tx_ids_and_sizes: Vec<TxIdAndSize<EraTxId>> = expected_ids
            .iter()
            .map(|&i| TxIdAndSize(era_tx_ids[i].clone(), 32))
            .collect();
        assert_next_message(rx_messages, ReplyTxIds(tx_ids_and_sizes)).await?;
        Ok(())
    }

    /// Check that the next message is a ReplyTxs with the expected transaction bodies.
    pub(crate) async fn assert_tx_bodies_reply(
        rx_messages: &mut Receiver<Message<EraTxId, EraTxBody>>,
        era_tx_bodies: &[EraTxBody],
        expected_body_ids: &[usize],
    ) -> anyhow::Result<()> {
        let txs: Vec<EraTxBody> = expected_body_ids
            .iter()
            .map(|&i| era_tx_bodies[i].clone())
            .collect();
        assert_next_message(rx_messages, ReplyTxs(txs)).await?;
        Ok(())
    }

    /// Check that the next message matches the expected one.
    pub(crate) async fn assert_next_message(
        rx_messages: &mut Receiver<Message<EraTxId, EraTxBody>>,
        expected: Message<EraTxId, EraTxBody>,
    ) -> anyhow::Result<()> {
        let actual = rx_messages
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("no message received"))?;
        let actual = MessageEq(actual);
        let expected = MessageEq(expected);
        assert_eq!(actual, expected, "actual = {actual}\nexpected = {expected}");
        Ok(())
    }

    /// Send a series of requests to a tx submission client backed by the given mempool.
    /// Returns a receiver to collect the messages that the client sent back.
    async fn start_client(
        mempool: Arc<dyn Mempool<Tx>>,
    ) -> anyhow::Result<(
        Sender<Request<EraTxId>>,
        Receiver<Message<EraTxId, EraTxBody>>,
        JoinHandle<anyhow::Result<()>>,
    )> {
        let mut client_sm = TxSubmissionClient::new(mempool.clone(), Peer::new("peer-1"));

        let (tx_req, rx_req) = mpsc::channel(10);
        let (tx_messages, rx_messages) = mpsc::channel(10);

        let transport = MockClientTransport {
            rx_req,
            tx_reply: tx_messages,
        };

        let client_handle =
            tokio::spawn(async move { client_sm.start_client_with_transport(transport).await });
        Ok((tx_req, rx_messages, client_handle))
    }

    /// Create a new EraTxId for the Conway era.
    pub(crate) fn new_era_tx_id(tx_id: TxId) -> EraTxId {
        EraTxId(Era::Conway.into(), tx_id.to_vec())
    }

    /// Create a new EraTxBody for the Conway era.
    pub(crate) fn new_era_tx_body(tx_body: Vec<u8>) -> EraTxBody {
        EraTxBody(Era::Conway.into(), tx_body)
    }

    struct MockClientTransport {
        // server -> client messages
        rx_req: Receiver<Request<EraTxId>>,
        // client -> server messages
        tx_reply: Sender<Message<EraTxId, EraTxBody>>,
    }

    fn era_tx_id_to_string(era_tx_id: &EraTxId) -> String {
        Hash::<32>::from(era_tx_id.1.as_slice()).to_string()
    }

    fn era_tx_body_to_string(era_tx_body: &EraTxBody) -> String {
        String::from_utf8(era_tx_body.1.clone()).unwrap()
    }

    #[async_trait::async_trait]
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
            self.rx_req
                .recv()
                .await
                .ok_or_else(|| anyhow::anyhow!("mock closed"))
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

    #[derive(Debug, PartialEq, Eq, Clone)]
    pub(crate) struct Tx {
        tx_body: String,
    }

    impl Tx {
        pub(crate) fn new(tx_body: impl Into<String>) -> Self {
            Self {
                tx_body: tx_body.into(),
            }
        }

        pub(crate) fn tx_id(&self) -> TxId {
            TxId::from(self.tx_body.as_str())
        }

        pub(crate) fn tx_body(&self) -> Vec<u8> {
            minicbor::to_vec(&self.tx_body).unwrap()
        }
    }

    impl CborLen<()> for Tx {
        fn cbor_len(&self, _ctx: &mut ()) -> usize {
            self.tx_body.len()
        }
    }

    impl Encode<()> for Tx {
        fn encode<W: Write>(
            &self,
            e: &mut Encoder<W>,
            _ctx: &mut (),
        ) -> Result<(), Error<W::Error>> {
            e.encode(&self.tx_body)?;
            Ok(())
        }
    }

    impl<'a> Decode<'a, ()> for Tx {
        fn decode(d: &mut Decoder<'a>, _ctx: &mut ()) -> Result<Self, minicbor::decode::Error> {
            let tx_body: String = d.decode()?;
            Ok(Tx { tx_body })
        }
    }

    pub(crate) struct MessageEq(Message<EraTxId, EraTxBody>);

    impl Debug for MessageEq {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self)
        }
    }

    impl Display for MessageEq {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match &self.0 {
                Message::Init => write!(f, "Init"),
                Message::RequestTxIds(blocking, ack, req) => {
                    write!(
                        f,
                        "RequestTxIds(blocking={}, ack={}, req={})",
                        blocking, ack, req
                    )
                }
                ReplyTxIds(ids_and_sizes) => {
                    let ids: Vec<String> = ids_and_sizes
                        .iter()
                        .map(|tx_id_and_size| era_tx_id_to_string(&tx_id_and_size.0))
                        .collect();
                    write!(f, "ReplyTxIds(ids=[{}])", ids.join(", "))
                }
                Message::RequestTxs(ids) => {
                    let ids_str: Vec<String> =
                        ids.iter().map(|tx_id| era_tx_id_to_string(tx_id)).collect();
                    write!(f, "RequestTxs(ids=[{}])", ids_str.join(", "))
                }
                ReplyTxs(bodies) => {
                    let bodies_str: Vec<String> = bodies
                        .iter()
                        .map(|tx_body| era_tx_body_to_string(tx_body))
                        .collect();
                    write!(f, "ReplyTxs(bodies=[{}])", bodies_str.join(", "))
                }
                Message::Done => write!(f, "Done"),
            }
        }
    }

    impl PartialEq for MessageEq {
        fn eq(&self, other: &Self) -> bool {
            match (&self.0, &other.0) {
                (Message::Done, Message::Done) => true,
                (Message::Init, Message::Init) => true,
                (Message::RequestTxIds(b1, a1, r1), Message::RequestTxIds(b2, a2, r2)) => {
                    b1 == b2 && a1 == a2 && r1 == r2
                }
                (Message::RequestTxs(ids1), Message::RequestTxs(ids2)) => {
                    ids1.into_iter()
                        .map(|id| EraTxIdOrd::new(id.clone()))
                        .collect::<Vec<_>>()
                        == ids2
                        .into_iter()
                        .map(|id| EraTxIdOrd::new(id.clone()))
                        .collect::<Vec<_>>()
                }
                (ReplyTxIds(ids1), ReplyTxIds(ids2)) => {
                    ids1.into_iter().map(|id| (id.0.0, id.0.1.clone(), id.1)).collect::<Vec<_>>()
                        == ids2.into_iter().map(|id| (id.0.0, id.0.1.clone(), id.1)).collect::<Vec<_>>()
                }
                (ReplyTxs(txs1), ReplyTxs(txs2)) => {
                    txs1.into_iter().cloned().map(|tx| (tx.0, tx.1)).collect::<Vec<_>>()
                        == txs2.into_iter().cloned().map(|tx| (tx.0, tx.1)).collect::<Vec<_>>()
                }
                _ => false,
            }
        }
    }
}
