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
        let transport = PallasTxSubmissionTransport::new(agent_channel);
        self.start_client_with_transport(transport).await
    }

    /// Core state machine, parameterized over a transport for testability.
    async fn start_client_with_transport<T: TxSubmissionTransport>(
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
                    self.provide_transactions(&mut transport, acknowledged, required_next).await?;
                }
                Request::TxIdsNonBlocking(acknowledged, required_next) => {
                    self.provide_transactions(&mut transport, acknowledged, required_next).await?;
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

    async fn provide_transactions<T: TxSubmissionTransport>(&mut self, transport: &mut T, acknowledged: u16, required_next: u16) -> anyhow::Result<()> {
        self.discard(acknowledged);
        let tx_ids = self.mempool.tx_ids_since(self.next_seq(), required_next);
        transport
            .reply_tx_ids(
                tx_ids
                    .iter()
                    .map(|(tx_id, tx_size, _)| {
                        TxIdAndSize(
                            EraTxId(Era::Conway.into(), tx_id.to_vec()),
                            *tx_size,
                        )
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
pub trait TxSubmissionTransport: Send {
    async fn send_init(&mut self) -> anyhow::Result<()>;
    async fn send_done(&mut self) -> anyhow::Result<()>;

    async fn next_request(&mut self) -> anyhow::Result<Request<EraTxId>>;

    async fn reply_tx_ids(&mut self, ids: Vec<TxIdAndSize<EraTxId>>) -> anyhow::Result<()>;
    async fn reply_txs(&mut self, txs: Vec<EraTxBody>) -> anyhow::Result<()>;
}

/// Production adapter around pallas' txsubmission client.
pub struct PallasTxSubmissionTransport {
    inner: txsubmission::Client,
}

impl PallasTxSubmissionTransport {
    pub fn new(agent_channel: AgentChannel) -> Self {
        Self {
            inner: txsubmission::Client::new(agent_channel),
        }
    }
}

#[async_trait]
impl TxSubmissionTransport for PallasTxSubmissionTransport {
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
mod tests {
    use super::*;
    use amaru_kernel::cbor::Encode;
    use amaru_mempool::strategies::InMemoryMempool;
    use minicbor::Encoder;
    use minicbor::encode::{Error, Write};
    use std::fmt::Display;
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
        let (tx_req, mut effects, _client_handle) = start_client(mempool).await?;

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
        assert_next_effect(&mut effects, Effect::InitSent).await?;
        assert_tx_ids_reply(&mut effects, &era_tx_ids, &[0, 1]).await?;
        assert_tx_bodies_reply(&mut effects, &era_tx_bodies, &[0, 1]).await?;
        assert_tx_ids_reply(&mut effects, &era_tx_ids, &[2, 3]).await?;
        assert_tx_bodies_reply(&mut effects, &era_tx_bodies, &[2, 3]).await?;
        assert_tx_ids_reply(&mut effects, &era_tx_ids, &[4]).await?;
        assert_tx_bodies_reply(&mut effects, &era_tx_bodies, &[4]).await?;

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
        let (tx_req, mut effects, _client_handle) = start_client(mempool.clone()).await?;

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

        assert_next_effect(&mut effects, Effect::InitSent).await?;
        assert_tx_ids_reply(&mut effects, &era_tx_ids, &[0, 1]).await?;
        assert_tx_bodies_reply(&mut effects, &era_tx_bodies, &[0, 1]).await?;
        assert_tx_ids_reply(&mut effects, &era_tx_ids, &[]).await?;

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
        assert_tx_ids_reply(&mut effects, &era_tx_ids, &[2, 3]).await?;
        assert_tx_bodies_reply(&mut effects, &era_tx_bodies, &[2, 3]).await?;
        assert_tx_ids_reply(&mut effects, &era_tx_ids, &[4]).await?;
        assert_tx_bodies_reply(&mut effects, &era_tx_bodies, &[4]).await?;

        Ok(())
    }

    // HELPERS

    /// Check that the next effect is a TxIdsReply with the expected ids.
    async fn assert_tx_ids_reply(
        rx_eff: &mut Receiver<Effect>,
        era_tx_ids: &[EraTxId],
        expected_ids: &[usize],
    ) -> anyhow::Result<()> {
        let tx_ids_and_sizes: Vec<TxIdAndSize<EraTxId>> = expected_ids
            .iter()
            .map(|&i| TxIdAndSize(era_tx_ids[i].clone(), 1))
            .collect();
        assert_next_effect(rx_eff, Effect::TxIdsReply(tx_ids_and_sizes)).await?;
        Ok(())
    }

    /// Check that the next effect is a TxsReply with the expected transaction bodies.
    async fn assert_tx_bodies_reply(
        rx_eff: &mut Receiver<Effect>,
        era_tx_bodies: &[EraTxBody],
        expected_body_ids: &[usize],
    ) -> anyhow::Result<()> {
        let txs: Vec<EraTxBody> = expected_body_ids.iter().map(|&i| era_tx_bodies[i].clone()).collect();
        assert_next_effect(rx_eff, Effect::TxsReply(txs)).await?;
        Ok(())
    }

    /// Check that the next effect matches the expected one.
    async fn assert_next_effect(
        rx_eff: &mut Receiver<Effect>,
        expected: Effect,
    ) -> anyhow::Result<()> {
        let actual = rx_eff
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("no effect received"))?;
        assert_eq!(actual, expected, "actual = {actual}\nexpected = {expected}");
        Ok(())
    }

    /// Send a series of requests to a tx submission client backed by the given mempool.
    /// Returns a receiver to collect the effects (what the client sent back).
    async fn start_client(
        mempool: Arc<dyn Mempool<Tx>>,
    ) -> anyhow::Result<(Sender<Request<EraTxId>>, Receiver<Effect>, JoinHandle<anyhow::Result<()>>)> {
        let mut client_sm = TxSubmissionClient::new(mempool.clone(), Peer::new("peer-1"));

        let (tx_req, rx_req) = mpsc::channel(10);
        let (tx_eff, rx_eff) = mpsc::channel(10);

        let transport = MockTransport {
            rx_req,
            tx_effect: tx_eff,
        };

        let client_handle = tokio::spawn(async move { client_sm.start_client_with_transport(transport).await });
        Ok((tx_req, rx_eff, client_handle))
    }

    /// Create a new EraTxId for the Conway era.
    fn new_era_tx_id(tx_id: TxId) -> EraTxId {
        EraTxId(Era::Conway.into(), tx_id.to_vec())
    }

    /// Create a new EraTxBody for the Conway era.
    fn new_era_tx_body(tx_body: Vec<u8>) -> EraTxBody {
        EraTxBody(Era::Conway.into(), tx_body)
    }

    struct MockTransport {
        // server -> client requests
        rx_req: Receiver<Request<EraTxId>>,
        // client -> server effects (what client sends)
        tx_effect: mpsc::Sender<Effect>,
    }

    fn era_tx_id_to_string(era_tx_id: &EraTxId) -> String {
        Hash::<32>::from(era_tx_id.1.as_slice()).to_string()
    }

    fn era_tx_body_to_string(era_tx_body: &EraTxBody) -> String {
        String::from_utf8(era_tx_body.1.clone()).unwrap()
    }

    #[derive(Debug)]
    enum Effect {
        InitSent,
        DoneSent,
        TxIdsReply(Vec<TxIdAndSize<EraTxId>>),
        TxsReply(Vec<EraTxBody>),
    }

    impl Display for Effect {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Effect::InitSent => write!(f, "InitSent"),
                Effect::DoneSent => write!(f, "DoneSent"),
                Effect::TxIdsReply(ids) => {
                    let ids_str = ids
                        .iter()
                        .map(|tx_id| era_tx_id_to_string(&tx_id.0))
                        .collect::<Vec<_>>()
                        .join(", ");
                    write!(f, "TxIdsReply([{}])", ids_str)
                }
                Effect::TxsReply(txs) => {
                    let txs_str = txs
                        .iter()
                        .map(|tx| era_tx_body_to_string(tx))
                        .collect::<Vec<_>>()
                        .join(", ");
                    write!(f, "TxsReply([{}])", txs_str)
                }
            }
        }
    }

    impl PartialEq for Effect {
        fn eq(&self, other: &Self) -> bool {
            match (self, other) {
                (Effect::InitSent, Effect::InitSent) => true,
                (Effect::DoneSent, Effect::DoneSent) => true,
                (Effect::TxIdsReply(a), Effect::TxIdsReply(b)) => {
                    a.iter()
                        .map(|tx_id| era_tx_id_to_string(&tx_id.0))
                        .collect::<Vec<_>>()
                        == b.iter()
                        .map(|tx_id| era_tx_id_to_string(&tx_id.0))
                        .collect::<Vec<_>>()
                }
                (Effect::TxsReply(a), Effect::TxsReply(b)) => {
                    a.iter()
                        .map(|tx_id| era_tx_body_to_string(tx_id))
                        .collect::<Vec<_>>()
                        == b.iter()
                        .map(|tx_id| era_tx_body_to_string(tx_id))
                        .collect::<Vec<_>>()
                }
                _ => false,
            }
        }
    }

    #[async_trait::async_trait]
    impl TxSubmissionTransport for MockTransport {
        async fn send_init(&mut self) -> anyhow::Result<()> {
            self.tx_effect.send(Effect::InitSent).await?;
            Ok(())
        }

        async fn send_done(&mut self) -> anyhow::Result<()> {
            self.tx_effect.send(Effect::DoneSent).await?;
            Ok(())
        }

        async fn next_request(&mut self) -> anyhow::Result<Request<EraTxId>> {
            self.rx_req
                .recv()
                .await
                .ok_or_else(|| anyhow::anyhow!("mock closed"))
        }

        async fn reply_tx_ids(&mut self, ids: Vec<TxIdAndSize<EraTxId>>) -> anyhow::Result<()> {
            self.tx_effect.send(Effect::TxIdsReply(ids)).await?;
            Ok(())
        }

        async fn reply_txs(&mut self, txs: Vec<EraTxBody>) -> anyhow::Result<()> {
            self.tx_effect.send(Effect::TxsReply(txs)).await?;
            Ok(())
        }
    }

    #[derive(Debug, PartialEq, Eq, Clone)]
    struct Tx {
        tx_body: String,
    }

    impl Tx {
        fn new(tx_body: impl Into<String>) -> Self {
            Self {
                tx_body: tx_body.into(),
            }
        }

        fn tx_id(&self) -> TxId {
            TxId::from(self.tx_body.as_str())
        }

        fn tx_body(&self) -> Vec<u8> {
            minicbor::to_vec(self).unwrap()
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
}
