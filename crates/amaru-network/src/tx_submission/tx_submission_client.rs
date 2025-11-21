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

use std::collections::{VecDeque};
use std::sync::Arc;
use minicbor::{CborLen, Encode};
use pallas_network::miniprotocols::txsubmission;
use pallas_network::miniprotocols::txsubmission::{EraTxBody, EraTxId, Request, TxIdAndSize};
use pallas_network::multiplexer::AgentChannel;
use pallas_traverse::Era;
use amaru_kernel::peer::Peer;
use amaru_kernel::{to_cbor, Hash};
use amaru_ouroboros_traits::{Mempool, MempoolSeqNo, TxId};


pub struct TxSubmissionClient<Tx: Send + Sync + 'static> {
    mempool: Arc<dyn Mempool<Tx>>,
    peer: Peer,
    state: ClientPeerState,
}

#[derive(Debug)]
struct ClientPeerState {
    /// What we’ve already advertised but has not yet been fully acked.
    window: VecDeque<(TxId, MempoolSeqNo)>,
    /// Last seq_no we have ever pulled from the mempool for this peer.
    last_seq: MempoolSeqNo,
}

impl ClientPeerState {
    /// We discard up to 'acknowledged' from our window.
    fn discard(&mut self, acknowledged: u16) {
        self.window.truncate(acknowledged as usize);
    }

    fn update(
        &mut self,
        tx_ids: Vec<(TxId, u32, MempoolSeqNo)>,
    ) {
        for (tx_id, _size, seq_no) in tx_ids {
            self.window.push_back((tx_id, seq_no));
            self.last_seq = seq_no;
        }
    }
}

impl<Tx: Encode<()> + CborLen<()> + Send + Sync + 'static> TxSubmissionClient<Tx> {
    pub fn new(mempool: Arc<dyn Mempool<Tx>>, peer: Peer) -> Self {
        Self {
            mempool: mempool.clone(),
            peer,
            state: ClientPeerState {
                window: VecDeque::new(),
                last_seq: MempoolSeqNo(0),
            },
        }
    }

    async fn start_client(&mut self, agent_channel: AgentChannel) -> anyhow::Result<()> {
        let mut client = txsubmission::Client::new(agent_channel);
        client.send_init().await?;
        loop {
            match client.next_request().await? {
                Request::TxIds(acknowledged, required_next) => {
                    self.state.discard(acknowledged);
                    if !self.mempool.wait_for_at_least(required_next).await {
                        client.send_done().await?;
                        break;
                    }

                    let tx_ids = self.mempool.tx_ids_since(self.state.last_seq, required_next);
                    client.reply_tx_ids(
                        tx_ids
                            .iter().map(|(tx_id, tx_size, _)| TxIdAndSize(EraTxId(Era::Conway.into(), tx_id.to_vec()), *tx_size))
                            .collect()
                    ).await?;
                    self.state.update(tx_ids);
                }
                Request::TxIdsNonBlocking(acknowledged, required_next) => {
                    self.state.discard(acknowledged);
                    let tx_ids = self.mempool.tx_ids_since(self.state.last_seq, required_next);
                    let tx_ids: Vec<TxIdAndSize<EraTxId>> = tx_ids.into_iter()
                        .map(|(tx_id, tx_size, _)| TxIdAndSize(EraTxId(Era::Conway.into(), tx_id.to_vec()), tx_size))
                        .collect();

                    client.reply_tx_ids(tx_ids).await?;
                }
                Request::Txs(ids) => {
                    let ids: Vec<TxId> = ids.into_iter().map(|tx_id| TxId::new(Hash::from(tx_id.1.as_slice()))).collect();
                    let txs = self.mempool.get_txs_for_ids(ids.as_slice());
                    client.reply_txs(txs
                        .into_iter().map(|tx| {
                        let tx_body = to_cbor(&*tx);
                        // TODO: make sure that we only need to support one era here
                        EraTxBody(Era::Conway.into(), tx_body)
                    })
                        .collect::<Vec<_>>()).await?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use minicbor::encode::{Error, Write};
    use minicbor::Encoder;
    use amaru_kernel::cbor::Encode;
    use amaru_mempool::strategies::{InMemoryMempool};
    use super::*;

    #[tokio::test]
    async fn serve_transactions() -> anyhow::Result<()> {
        let _mempool: Arc<InMemoryMempool<Tx>> = Arc::new(InMemoryMempool::default());
        Ok(())
    }

    // HELPERS

    #[derive(Debug, PartialEq, Eq, Clone)]
    struct Tx(String, Vec<u8>);

    impl Encode<()> for Tx {
        fn encode<W: Write>(&self, e: &mut Encoder<W>, _ctx: &mut ()) -> Result<(), Error<W::Error>> {
            e.encode(&self.1)?;
            Ok(())
        }
    }
}
