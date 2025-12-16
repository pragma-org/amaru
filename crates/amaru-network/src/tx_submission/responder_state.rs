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

use crate::tx_submission::ProtocolError::{
    ReceivedTxsExceedsBatchSize, SomeReceivedTxsNotInFlight, TooManyTxIdsReceived,
};
use crate::tx_submission::TxSubmissionState::{TxIdsBlocking, TxIdsNonBlocking, Txs};
use crate::tx_submission::{
    Blocking, Message, Outcome, ResponderParams, TxSubmissionState, protocol_error,
};
use amaru_kernel::Tx;
use amaru_ouroboros_traits::{TxId, TxOrigin, TxSubmissionMempool};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, VecDeque};

/// State of a transaction submission responder for a given peer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TxSubmissionResponderState {
    /// Responder parameters: batch sizes, window sizes, etc.
    params: ResponderParams,
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
}

impl TxSubmissionResponderState {
    pub fn new(params: ResponderParams) -> Self {
        Self {
            params,
            window: VecDeque::new(),
            pending_fetch: VecDeque::new(),
            inflight_fetch_queue: VecDeque::new(),
            inflight_fetch_set: BTreeSet::new(),
        }
    }

    pub async fn step(
        &mut self,
        mempool: &dyn TxSubmissionMempool<Tx>,
        state: &TxSubmissionState,
        input: Message,
    ) -> anyhow::Result<(TxSubmissionState, Outcome)> {
        Ok(match (state, input) {
            (TxSubmissionState::Init, Message::Init) => {
                tracing::trace!("received Init");
                self.initialize_state(mempool)
            }
            (TxIdsBlocking | TxIdsNonBlocking, Message::ReplyTxIds(tx_ids)) => {
                tracing::trace!("received ReplyTxIds");
                self.process_tx_ids_reply(mempool, tx_ids)?
            }
            (Txs, Message::ReplyTxs(txs)) => {
                tracing::trace!("received ReplyTxs");
                self.process_txs_reply(mempool, txs)?
            }
            (_, Message::Done) => {
                tracing::trace!("done");
                (TxSubmissionState::Done, Outcome::Done)
            }
            (_, msg) => {
                tracing::warn!("invalid state transition: {:?} <- {:?}", state, msg);
                return Err(anyhow::anyhow!(
                    "Invalid state transition: {:?} <- {:?}",
                    state,
                    msg
                ));
            }
        })
    }

    fn initialize_state(
        &mut self,
        mempool: &dyn TxSubmissionMempool<Tx>,
    ) -> (TxSubmissionState, Outcome) {
        let (ack, req, blocking) = self.request_tx_ids(mempool);
        (
            TxIdsBlocking,
            Outcome::Send(Message::RequestTxIds(ack, req, blocking)),
        )
    }

    fn process_tx_ids_reply(
        &mut self,
        mempool: &dyn TxSubmissionMempool<Tx>,
        tx_ids: Vec<(TxId, u32)>,
    ) -> anyhow::Result<(TxSubmissionState, Outcome)> {
        if tx_ids.len() > self.params.max_window.into() {
            return Ok(protocol_error(TooManyTxIdsReceived(
                tx_ids.len(),
                self.params.max_window.into(),
            )));
        }
        self.received_tx_ids(mempool, tx_ids);
        Ok((
            Txs,
            Outcome::Send(Message::RequestTxs(self.txs_to_request())),
        ))
    }

    fn process_txs_reply(
        &mut self,
        mempool: &dyn TxSubmissionMempool<Tx>,
        txs: Vec<Tx>,
    ) -> anyhow::Result<(TxSubmissionState, Outcome)> {
        if txs.len() > self.params.fetch_batch.into() {
            return Ok(protocol_error(ReceivedTxsExceedsBatchSize(
                txs.len(),
                self.params.fetch_batch.into(),
            )));
        }
        let tx_ids = txs.iter().map(TxId::from).collect::<Vec<_>>();
        let not_in_flight = tx_ids
            .iter()
            .filter(|tx_id| !self.inflight_fetch_set.contains(tx_id))
            .cloned()
            .collect::<Vec<_>>();
        if !not_in_flight.is_empty() {
            return Ok(protocol_error(SomeReceivedTxsNotInFlight(not_in_flight)));
        }

        self.received_txs(mempool, txs)?;
        let (ack, req, blocking) = self.request_tx_ids(mempool);
        let new_state = if blocking == Blocking::Yes {
            TxIdsBlocking
        } else {
            TxIdsNonBlocking
        };
        Ok((
            new_state,
            Outcome::Send(Message::RequestTxIds(ack, req, blocking)),
        ))
    }

    /// Prepare a request for tx ids, acknowledging already processed ones
    /// and requesting as many as fit in the window.
    #[allow(clippy::expect_used)]
    fn request_tx_ids(&mut self, mempool: &dyn TxSubmissionMempool<Tx>) -> (u16, u16, Blocking) {
        // Acknowledge everything we’ve already processed.
        let mut ack = 0_u16;

        while let Some((tx_id, _size)) = self.window.front() {
            let already_in_mempool = mempool.contains(tx_id);
            if already_in_mempool {
                // pop from window and ack it
                if self.window.pop_front().is_some() {
                    ack = ack
                        .checked_add(1)
                        .expect("ack overflow: protocol invariant violated");
                }
            } else {
                break;
            }
        }

        // Request as many as we can fit in the window.
        let req = self
            .params
            .max_window
            .checked_sub(self.window.len() as u16)
            .expect("req underflow: protocol invariant violated");

        // We need to block if there are no more outstanding tx ids.
        let blocking = if self.window.is_empty() {
            Blocking::Yes
        } else {
            Blocking::No
        };
        (ack, req, blocking)
    }

    /// Register received tx ids, adding them to the window and to the pending fetch list
    /// if they are not already in the mempool.
    fn received_tx_ids<Tx: Send + Sync + 'static>(
        &mut self,
        mempool: &dyn TxSubmissionMempool<Tx>,
        tx_ids: Vec<(TxId, u32)>,
    ) {
        for (tx_id, size) in tx_ids {
            // We add the tx id to the window to acknowledge it on the next round.
            self.window.push_back((tx_id, size));

            // We only add to pending fetch if we haven't received it yet in the mempool.
            if !mempool.contains(&tx_id) {
                self.pending_fetch.push_back(tx_id);
            }
        }
    }

    /// Prepare a batch of tx ids for the txs to request.
    fn txs_to_request(&mut self) -> Vec<TxId> {
        let mut tx_ids = Vec::new();

        while tx_ids.len() < self.params.fetch_batch.into() {
            if let Some(id) = self.pending_fetch.pop_front() {
                self.inflight_fetch_queue.push_back(id);
                self.inflight_fetch_set.insert(id);
                tx_ids.push(id);
            } else {
                break;
            }
        }

        tx_ids
    }

    /// Process received txs, validating and inserting them into the mempool.
    fn received_txs(
        &mut self,
        mempool: &dyn TxSubmissionMempool<Tx>,
        txs: Vec<Tx>,
    ) -> anyhow::Result<()> {
        for tx in txs {
            // this is the exact id we requested for this body (FIFO)
            if let Some(requested_id) = self.inflight_fetch_queue.pop_front() {
                self.inflight_fetch_set.remove(&requested_id);

                match mempool.validate_transaction(tx.clone()) {
                    Ok(_) => {
                        mempool.insert(tx, TxOrigin::Remote)?;
                    }
                    Err(e) => {
                        tracing::warn!("received invalid transaction {}: {}", requested_id, e);
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tx_submission::Message::*;
    use crate::tx_submission::Outcome::{Error, Send};
    use crate::tx_submission::tests::{create_transactions, reply_tx_ids, reply_txs, request_txs};
    use crate::tx_submission::{assert_outcomes_eq, request_tx_ids};
    use amaru_mempool::strategies::InMemoryMempool;
    use pallas_primitives::conway::Tx;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_responder() -> anyhow::Result<()> {
        let txs = create_transactions(6);

        // Create a mempool with no initial transactions
        // since we are going to fetch them from the initiator
        let mempool = Arc::new(InMemoryMempool::default());

        // Send replies from the initiator as if they were replies to previous requests from the responder
        let messages = vec![
            Init,
            reply_tx_ids(&txs, &[0, 1, 2]),
            reply_txs(&txs, &[0, 1]),
            reply_tx_ids(&txs, &[3, 4, 5]),
            reply_txs(&txs, &[2, 3]),
            reply_tx_ids(&txs, &[]),
            reply_txs(&txs, &[4, 5]),
            Done,
        ];

        let outcomes = run_messages(mempool.clone(), messages).await?;

        assert_outcomes_eq(
            &outcomes,
            &[
                Send(request_tx_ids(0, 3, Blocking::Yes)),
                Send(request_txs(&txs, &[0, 1])),
                Send(request_tx_ids(2, 2, Blocking::No)),
                Send(request_txs(&txs, &[2, 3])),
                Send(request_tx_ids(2, 1, Blocking::No)),
                Send(request_txs(&txs, &[4, 5])),
                Send(request_tx_ids(2, 3, Blocking::Yes)),
                Outcome::Done,
            ],
        );
        Ok(())
    }

    #[tokio::test]
    async fn the_returned_tx_ids_should_respect_the_window_size() -> anyhow::Result<()> {
        let txs = create_transactions(6);
        let mempool = Arc::new(InMemoryMempool::default());

        let messages = vec![Init, reply_tx_ids(&txs, &[0, 1, 2, 3, 4])];

        let outcomes = run_messages(mempool.clone(), messages).await?;
        assert_outcomes_eq(
            &outcomes,
            &[
                Send(request_tx_ids(0, 3, Blocking::Yes)),
                Error(TooManyTxIdsReceived(5, 3)),
            ],
        );
        Ok(())
    }

    #[tokio::test]
    async fn the_returned_txs_should_respect_the_batch_size() -> anyhow::Result<()> {
        let txs = create_transactions(6);
        let mempool = Arc::new(InMemoryMempool::default());

        let messages = vec![
            Init,
            reply_tx_ids(&txs, &[0, 1, 2]),
            reply_txs(&txs, &[0]),
            reply_tx_ids(&txs, &[]),
            reply_txs(&txs, &[1, 2, 3]),
        ];

        let outcomes = run_messages(mempool.clone(), messages).await?;
        assert_outcomes_eq(
            &outcomes,
            &[
                Send(request_tx_ids(0, 3, Blocking::Yes)),
                Send(request_txs(&txs, &[0, 1])),
                Send(request_tx_ids(1, 1, Blocking::No)),
                Send(request_txs(&txs, &[2])),
                Error(ReceivedTxsExceedsBatchSize(3, 2)),
            ],
        );
        Ok(())
    }

    #[tokio::test]
    async fn the_returned_txs_be_a_subset_of_the_inflight_txs() -> anyhow::Result<()> {
        let txs = create_transactions(6);
        let mempool = Arc::new(InMemoryMempool::default());

        let messages = vec![
            Init,
            reply_tx_ids(&txs, &[0, 1, 2]),
            reply_txs(&txs, &[0]),
            reply_tx_ids(&txs, &[]),
            reply_txs(&txs, &[1, 3]),
        ];

        let outcomes = run_messages(mempool.clone(), messages).await?;
        assert_outcomes_eq(
            &outcomes,
            &[
                Send(request_tx_ids(0, 3, Blocking::Yes)),
                Send(request_txs(&txs, &[0, 1])),
                Send(request_tx_ids(1, 1, Blocking::No)),
                Send(request_txs(&txs, &[2])),
                Error(SomeReceivedTxsNotInFlight(vec![TxId::from(&txs[3])])),
            ],
        );
        Ok(())
    }

    // HELPERS

    async fn run_messages(
        mempool: Arc<dyn TxSubmissionMempool<Tx>>,
        requests: Vec<Message>,
    ) -> anyhow::Result<Vec<Outcome>> {
        let (outcomes, _tx_state, _responder) = run_messages_state(mempool, requests).await?;
        Ok(outcomes)
    }

    async fn run_messages_state(
        mempool: Arc<dyn TxSubmissionMempool<Tx>>,
        requests: Vec<Message>,
    ) -> anyhow::Result<(Vec<Outcome>, TxSubmissionState, TxSubmissionResponderState)> {
        run_messages_state_with(
            TxSubmissionState::Init,
            TxSubmissionResponderState::new(ResponderParams::default()),
            mempool,
            requests,
        )
        .await
    }

    async fn run_messages_state_with(
        mut tx_state: TxSubmissionState,
        mut responder: TxSubmissionResponderState,
        mempool: Arc<dyn TxSubmissionMempool<Tx>>,
        requests: Vec<Message>,
    ) -> anyhow::Result<(Vec<Outcome>, TxSubmissionState, TxSubmissionResponderState)> {
        let mut outcomes = vec![];
        for r in requests {
            let (new_tx_state, outcome) = responder.step(mempool.as_ref(), &tx_state, r).await?;
            tx_state = new_tx_state;
            outcomes.push(outcome);
        }
        Ok((outcomes, tx_state, responder))
    }
}
