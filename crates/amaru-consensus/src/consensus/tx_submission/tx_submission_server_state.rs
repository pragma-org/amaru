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

use crate::consensus::tx_submission::{Blocking, ServerParams};
use amaru_kernel::peer::Peer;
use amaru_ouroboros_traits::{TxId, TxOrigin, TxSubmissionMempool};
use pallas_primitives::conway::Tx;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, VecDeque};
use std::sync::Arc;

/// State of a transaction submission server for a given peer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxSubmissionServerState {
    /// Server parameters: batch sizes, window sizes, etc.
    params: ServerParams,
    /// Peer we are connecting to.
    peer: Peer,
    /// All tx_ids advertised but not yet acked (and their size).
    window: VecDeque<(TxId, u32)>,
    /// Tx ids we want to fetch but haven't yet requested.
    pending_fetch: VecDeque<TxId>,
    /// Tx ids we requested; waiting for replies.
    /// First as a FIFO queue because when we receive tx bodies we don't get the ids back.
    inflight_fetch_queue: VecDeque<TxId>,
    /// Then as a set for quick lookup when processing received ids.
    /// This is kept in sync with `inflight_fetch_queue`. When we receive a tx body,
    /// we pop it from the front of the queue and remove it from the set.
    inflight_fetch_set: BTreeSet<TxId>,
    /// Tx ids we processed but didn't insert (invalid, policy failure, etc.).
    rejected: BTreeSet<TxId>,
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

    pub fn peer(&self) -> &Peer {
        &self.peer
    }

    pub fn process_tx_ids_reply(
        &mut self,
        mempool: Arc<dyn TxSubmissionMempool<Tx>>,
        tx_ids: Vec<(TxId, u32)>,
    ) -> anyhow::Result<Option<Vec<TxId>>> {
        self.received_tx_ids(mempool, tx_ids)?;
        Ok(self.txs_to_request())
    }

    pub async fn process_txs_reply(
        &mut self,
        mempool: Arc<dyn TxSubmissionMempool<Tx>>,
        txs: Vec<Tx>,
    ) -> anyhow::Result<(u16, u16, Blocking)> {
        self.received_txs(mempool.clone(), txs).await?;
        self.request_tx_ids(mempool.clone()).await
    }

    pub async fn request_tx_ids(
        &mut self,
        mempool: Arc<dyn TxSubmissionMempool<Tx>>,
    ) -> anyhow::Result<(u16, u16, Blocking)> {
        // Acknowledge everything we’ve already processed.
        let mut ack = 0_u16;

        while let Some((tx_id, _size)) = self.window.front() {
            let already_in_mempool = mempool.contains(tx_id);
            let already_rejected = self.rejected.contains(tx_id);

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

        // We need to block if there are no more outstanding tx ids.
        let blocking = if self.window.is_empty() {
            Blocking::Yes
        } else {
            Blocking::No
        };
        Ok((ack, req, blocking))
    }

    pub fn received_tx_ids<Tx: Send + Sync + 'static>(
        &mut self,
        mempool: Arc<dyn TxSubmissionMempool<Tx>>,
        tx_ids: Vec<(TxId, u32)>,
    ) -> anyhow::Result<()> {
        if tx_ids.len() > self.params.max_window {
            return Err(anyhow::anyhow!("Too many transactions ids received"));
        }

        for tx_id_and_size in tx_ids {
            let (tx_id, size) = (tx_id_and_size.0, tx_id_and_size.1);
            // We add the tx id to the window to acknowledge it on the next round.
            self.window.push_back((tx_id.clone(), size));

            // We only add to pending fetch if we haven't received it yet in the mempool.
            // and the tx id is not already rejected.
            if !mempool.contains(&tx_id) && !self.rejected.contains(&tx_id) {
                self.pending_fetch.push_back(tx_id);
            }
        }

        Ok(())
    }

    pub fn txs_to_request(&mut self) -> Option<Vec<TxId>> {
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

    pub async fn received_txs(
        &mut self,
        mempool: Arc<dyn TxSubmissionMempool<Tx>>,
        txs: Vec<Tx>,
    ) -> anyhow::Result<()> {
        if txs.len() > self.params.fetch_batch {
            return Err(anyhow::anyhow!(
                "Too many transactions received in one batch"
            ));
        }

        for tx in txs {
            // this is the exact id we requested for this body (FIFO)
            if let Some(requested_id) = self.inflight_fetch_queue.pop_front() {
                self.inflight_fetch_set.remove(&requested_id);

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
