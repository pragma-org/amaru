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
use amaru_ouroboros_traits::{MempoolSeqNo, TxId, TxServerRequest, TxSubmissionMempool};
use anyhow::anyhow;
use pallas_primitives::conway::Tx;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fmt::Debug;
use tracing::debug;

/// State of a transaction submission client for a given peer.
///
/// We keep track of:
///
///  - Which transaction ids have been advertised to the peer but not yet fully acknowledged.
///  - The last sequence number we pulled from the mempool for this peer, so we know where to continue from.
///
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

    pub fn peer(&self) -> &Peer {
        &self.peer
    }

    /// Process a request from the tx submission server
    pub async fn process_tx_server_request(
        &mut self,
        mempool: &dyn TxSubmissionMempool<Tx>,
        request: TxServerRequest,
    ) -> anyhow::Result<TxClientResponse<Tx>> {
        match request {
            TxServerRequest::TxIds { ack, req, .. } => {
                if req == 0 {
                    debug!(peer = %self.peer,
                        "Requested 0 tx ids, terminating tx submission client",
                    );
                    return Ok(TxClientResponse::<Tx>::ProtocolError(anyhow!(
                        "0 transaction ids requested"
                    )));
                }
                // update the window by discarding acknowledged tx ids
                // and update the last_seq
                self.discard(ack);
                if !mempool
                    .wait_for_at_least(self.last_seq.unwrap_or_default().add(req as u64))
                    .await
                {
                    return Ok(TxClientResponse::<Tx>::Done);
                }
                let tx_ids = self.get_next_tx_ids(mempool, req)?;
                Ok(TxClientResponse::NextIds(tx_ids))
            }
            TxServerRequest::TxIdsNonBlocking { ack, req, .. } => {
                // update the window by discarding acknowledged tx ids
                // and update the last_seq
                self.discard(ack);
                Ok(TxClientResponse::NextIds(
                    self.get_next_tx_ids(mempool, req)?,
                ))
            }
            TxServerRequest::Txs { tx_ids, .. } => {
                if tx_ids.is_empty() {
                    debug!(peer = %self.peer,
                        "Requested 0 txs, terminating tx submission client"
                    );
                    return Ok(TxClientResponse::<Tx>::ProtocolError(anyhow!(
                        "0 transactions requested"
                    )));
                }
                if tx_ids
                    .iter()
                    .any(|id| !self.window.iter().any(|(wid, _)| wid == id))
                {
                    debug!(peer = %self.peer,
                        "Requested unknown tx ids, terminating tx submission client"
                    );
                    return Ok(TxClientResponse::<Tx>::ProtocolError(anyhow!(
                        "unadvertised transaction ids requested: {:?}",
                        tx_ids
                    )));
                }
                let txs = mempool.get_txs_for_ids(tx_ids.as_slice());
                if txs.is_empty() {
                    Ok(TxClientResponse::<Tx>::ProtocolError(anyhow!(
                        "unknown transactions requested: {:?}",
                        tx_ids
                    )))
                } else {
                    Ok(TxClientResponse::NextTxs(txs))
                }
            }
        }
    }

    /// Take notice of the acknowledged transactions, and send the next batch of tx ids.
    fn get_next_tx_ids<Tx: Send + Debug + Sync + 'static>(
        &mut self,
        mempool: &dyn TxSubmissionMempool<Tx>,
        required_next: u16,
    ) -> anyhow::Result<Vec<(TxId, u32)>> {
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

pub enum TxClientResponse<Tx> {
    Done,
    ProtocolError(anyhow::Error),
    NextIds(Vec<(TxId, u32)>),
    NextTxs(Vec<Tx>),
}
