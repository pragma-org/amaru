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
    BlockingRequestMadeWhenTxsStillUnacknowledged, MaxOutstandingTxIdsRequested,
    NoAckOrReqTxIdsRequested, NoTxIdsRequested, NoTxsRequested,
    NonBlockingRequestMadeWhenAllTxsAcknowledged, TooManyAcknowledgedTxs,
    UnadvertisedTransactionIdsRequested, UnknownTxsRequested,
};
use crate::tx_submission::{Blocking, Message, Outcome, ProtocolError, TxSubmissionState};
use TxSubmissionState::*;
use amaru_kernel::Tx;
use amaru_ouroboros_traits::{MempoolSeqNo, TxId, TxSubmissionMempool};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fmt::Debug;

const MAX_REQUESTED_TX_IDS: u16 = 10;

/// State of a transaction submission initiator for a given peer.
///
/// We keep track of:
///
///  - Which transaction ids have been advertised to the peer but not yet fully acknowledged.
///  - The last sequence number we pulled from the mempool for this peer, so we know where to continue from.
///
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TxSubmissionInitiatorState {
    /// What we’ve already advertised but has not yet been fully acked.
    window: VecDeque<(TxId, MempoolSeqNo)>,
    /// Last seq_no we have ever pulled from the mempool for this peer.
    /// None if we have not pulled anything yet.
    last_seq: Option<MempoolSeqNo>,
}

impl TxSubmissionInitiatorState {
    pub fn new() -> Self {
        Self {
            window: VecDeque::new(),
            last_seq: None,
        }
    }

    pub(crate) async fn step(
        &mut self,
        mempool: &dyn TxSubmissionMempool<Tx>,
        state: &TxSubmissionState,
        input: Message,
    ) -> anyhow::Result<(TxSubmissionState, Outcome)> {
        Ok(match (state, input) {
            (Init, Message::Init) => (Idle, Outcome::Send(Message::Init)),
            (Idle, Message::RequestTxIds(ack, req, Blocking::Yes)) => {
                self.request_tx_ids_blocking(mempool, ack, req).await?
            }
            (Idle, Message::RequestTxIds(ack, req, Blocking::No)) => {
                self.request_tx_ids_non_blocking(mempool, ack, req)?
            }
            (Idle, Message::RequestTxs(tx_ids)) => self.request_txs(mempool, tx_ids),
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }

    async fn request_tx_ids_blocking(
        &mut self,
        mempool: &dyn TxSubmissionMempool<Tx>,
        ack: u16,
        req: u16,
    ) -> anyhow::Result<(TxSubmissionState, Outcome)> {
        // check the ack and req values
        tracing::trace!(?ack, ?req, "received RequestTxIdsBlocking");
        if req == 0 {
            return Ok(protocol_error(NoTxIdsRequested));
        };
        if let Some(value) = self.check_ack_req(ack, req) {
            return Ok(value);
        }
        if (ack as usize) < self.window.len() {
            return Ok(protocol_error(
                BlockingRequestMadeWhenTxsStillUnacknowledged,
            ));
        }

        // update the window by discarding acknowledged tx ids and update the last_seq
        self.discard(ack);
        if !mempool
            .wait_for_at_least(self.last_seq.unwrap_or_default().add(req as u64))
            .await
        {
            return Ok((Done, Outcome::Done));
        }
        let tx_ids = self.get_next_tx_ids(mempool, req)?;
        Ok(send(Message::ReplyTxIds(tx_ids)))
    }

    fn request_tx_ids_non_blocking(
        &mut self,
        mempool: &dyn TxSubmissionMempool<Tx>,
        ack: u16,
        req: u16,
    ) -> anyhow::Result<(TxSubmissionState, Outcome)> {
        // check the ack and req values
        tracing::trace!(?ack, ?req, "received RequestTxIdsNonBlocking");
        if ack == 0 && req == 0 {
            return Ok(protocol_error(NoAckOrReqTxIdsRequested));
        }
        if let Some(value) = self.check_ack_req(ack, req) {
            return Ok(value);
        }
        if ack as usize == self.window.len() {
            return Ok(protocol_error(NonBlockingRequestMadeWhenAllTxsAcknowledged));
        }

        // update the window by discarding acknowledged tx ids and update the last_seq
        self.discard(ack);
        Ok(send(Message::ReplyTxIds(
            self.get_next_tx_ids(mempool, req)?,
        )))
    }

    fn request_txs(
        &mut self,
        mempool: &dyn TxSubmissionMempool<Tx>,
        tx_ids: Vec<TxId>,
    ) -> (TxSubmissionState, Outcome) {
        tracing::trace!(?tx_ids, "received RequestTxs");
        if tx_ids.is_empty() {
            return protocol_error(NoTxsRequested);
        }
        if tx_ids
            .iter()
            .any(|id| !self.window.iter().any(|(wid, _)| wid == id))
        {
            return protocol_error(UnadvertisedTransactionIdsRequested(tx_ids));
        }
        let txs = mempool.get_txs_for_ids(tx_ids.as_slice());
        if txs.is_empty() {
            protocol_error(UnknownTxsRequested(tx_ids))
        } else {
            send(Message::ReplyTxs(txs))
        }
    }

    /// Check that the ack and req values are valid for a request whether it is blocking or non blocking.
    fn check_ack_req(&mut self, ack: u16, req: u16) -> Option<(TxSubmissionState, Outcome)> {
        if req > MAX_REQUESTED_TX_IDS {
            Some(protocol_error(MaxOutstandingTxIdsRequested(
                req,
                MAX_REQUESTED_TX_IDS,
            )))
        } else if ack as usize > self.window.len() {
            Some(protocol_error(TooManyAcknowledgedTxs(
                ack,
                self.window.len() as u16,
            )))
        } else {
            None
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

    /// We discard up to 'acknowledged' transactions from our window, in a FIFO manner.
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

/// Helper to create a protocol error outcome and transition to Idle state.
pub fn protocol_error(error: ProtocolError) -> (TxSubmissionState, Outcome) {
    tracing::warn!("protocol error: {error}");
    (Done, Outcome::Error(error))
}

/// Helper to create a message to send and transition to Idle state.
fn send(msg: Message) -> (TxSubmissionState, Outcome) {
    (Idle, Outcome::Send(msg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tx_submission::Message::{Init, RequestTxIds, RequestTxs};
    use crate::tx_submission::Outcome::{Error, Send};
    use crate::tx_submission::tests::{SizedMempool, create_transactions, reply_tx_ids, reply_txs};
    use crate::tx_submission::{
        assert_outcomes_eq, create_transactions_in_mempool, request_tx_ids, request_txs,
    };
    use std::sync::Arc;

    #[tokio::test]
    async fn serve_transactions() -> anyhow::Result<()> {
        // Create a mempool with some transactions
        let mempool = Arc::new(SizedMempool::with_capacity(6));
        let txs = create_transactions_in_mempool(mempool.clone(), 6);

        // Send requests to retrieve transactions and block until they are available.
        // In this case they are immediately available since we pre-populated the mempool.
        // Note that we acknowledge tx[0] first in a non-blocking request (line 1), then tx[1],
        // in the next blocking request (line 2).
        let messages = vec![
            Init,
            request_tx_ids(0, 2, Blocking::Yes),
            request_txs(&txs, &[0, 1]),
            request_tx_ids(1, 2, Blocking::No), // line 1
            request_txs(&txs, &[2, 3]),
            request_tx_ids(3, 2, Blocking::Yes), // line 2
            request_txs(&txs, &[4, 5]),
            request_tx_ids(2, 2, Blocking::Yes),
        ];

        let outcomes = run_messages(mempool, messages).await?;

        // Check replies
        // We basically assert that we receive the expected ids and transactions
        // 2 by 2, then the last one, since we requested batches of 2.
        assert_outcomes_eq(
            &outcomes,
            &[
                Send(Init),
                Send(reply_tx_ids(&txs, &[0, 1])),
                Send(reply_txs(&txs, &[0, 1])),
                Send(reply_tx_ids(&txs, &[2, 3])),
                Send(reply_txs(&txs, &[2, 3])),
                Send(reply_tx_ids(&txs, &[4, 5])),
                Send(reply_txs(&txs, &[4, 5])),
                Outcome::Done,
            ],
        );

        Ok(())
    }

    #[tokio::test]
    async fn serve_transactions_with_mempool_refilling() -> anyhow::Result<()> {
        // Create a mempool with some transactions
        let mempool = Arc::new(SizedMempool::with_capacity(6));
        let txs = create_transactions(6);

        for tx in txs.iter().take(2) {
            mempool.add(tx.clone())?;
        }

        // Send requests to retrieve transactions and block until they are available.
        // In this case they are immediately available since we pre-populated the mempool.
        let messages = vec![
            Init,
            request_tx_ids(0, 2, Blocking::Yes),
            request_txs(&txs, &[0, 1]),
            request_tx_ids(1, 2, Blocking::No),
        ];

        let (outcomes, tx_state, initiator) = run_messages_state(mempool.clone(), messages).await?;
        assert_outcomes_eq(
            &outcomes,
            &[
                Send(Init),
                Send(reply_tx_ids(&txs, &[0, 1])),
                Send(reply_txs(&txs, &[0, 1])),
                Send(reply_tx_ids(&txs, &[])),
            ],
        );

        // Refill the mempool with more transactions
        for tx in &txs[2..] {
            mempool.add(tx.clone())?;
        }
        let messages = vec![
            request_tx_ids(1, 2, Blocking::Yes),
            request_txs(&txs, &[2, 3]),
            request_tx_ids(2, 2, Blocking::Yes),
            request_txs(&txs, &[4, 5]),
            request_tx_ids(2, 2, Blocking::Yes),
        ];

        let (outcomes, _, _) =
            run_messages_state_with(tx_state, initiator, mempool, messages).await?;

        // Check replies
        // We basically assert that we receive the expected ids and transactions
        // 2 by 2, then the last one, since we requested batches of 2.
        assert_outcomes_eq(
            &outcomes,
            &[
                Send(reply_tx_ids(&txs, &[2, 3])),
                Send(reply_txs(&txs, &[2, 3])),
                Send(reply_tx_ids(&txs, &[4, 5])),
                Send(reply_txs(&txs, &[4, 5])),
                Outcome::Done,
            ],
        );
        Ok(())
    }

    #[tokio::test]
    async fn request_txs_must_come_from_requested_ids() -> anyhow::Result<()> {
        // Create a mempool with some transactions
        let mempool = Arc::new(SizedMempool::with_capacity(6));
        let txs = create_transactions_in_mempool(mempool.clone(), 4);

        // Send requests to retrieve transactions and block until they are available.
        // In this case they are immediately available since we pre-populated the mempool.
        // The reply to the first message will be tx ids 0 and 1, which means that the responder
        // should then request transactions for those ids only.
        // In this test we receive a request for tx ids 2 and 3, which were not advertised yet,
        // so the initiator should terminate the session.
        let messages = vec![
            Init,
            request_tx_ids(0, 2, Blocking::Yes),
            request_txs(&txs, &[2, 3]),
        ];

        let outcomes = run_messages(mempool, messages).await?;
        assert_outcomes_eq(
            &outcomes,
            &[
                Send(Init),
                Send(reply_tx_ids(&txs, &[0, 1])),
                Error(UnadvertisedTransactionIdsRequested(vec![
                    TxId::from(&txs[2]),
                    TxId::from(&txs[3]),
                ])),
            ],
        );
        Ok(())
    }

    #[tokio::test]
    async fn blocking_requested_ids_must_be_greater_than_0() -> anyhow::Result<()> {
        let mempool = Arc::new(SizedMempool::with_capacity(6));

        let messages = vec![Init, RequestTxIds(0, 0, Blocking::Yes)];
        let outcomes = run_messages(mempool, messages).await?;
        assert_outcomes_eq(&outcomes, &[Send(Init), Error(NoTxIdsRequested)]);
        Ok(())
    }

    #[tokio::test]
    async fn blocking_requested_txs_must_be_greater_than_0() -> anyhow::Result<()> {
        let mempool = Arc::new(SizedMempool::with_capacity(4));
        let txs = create_transactions_in_mempool(mempool.clone(), 4);

        let messages = vec![Init, RequestTxIds(0, 2, Blocking::Yes), RequestTxs(vec![])];

        let outcomes = run_messages(mempool, messages).await?;
        assert_outcomes_eq(
            &outcomes,
            &[
                Send(Init),
                Send(reply_tx_ids(&txs, &[0, 1])),
                Error(NoTxsRequested),
            ],
        );
        Ok(())
    }

    #[tokio::test]
    async fn non_blocking_ack_or_requested_ids_must_be_greater_than_0() -> anyhow::Result<()> {
        let mempool = Arc::new(SizedMempool::with_capacity(6));

        let messages = vec![Init, RequestTxIds(0, 0, Blocking::No)];
        let outcomes = run_messages(mempool, messages).await?;
        assert_outcomes_eq(&outcomes, &[Send(Init), Error(NoAckOrReqTxIdsRequested)]);
        Ok(())
    }

    #[tokio::test]
    async fn blocking_requested_nb_must_be_less_than_protocol_limit() -> anyhow::Result<()> {
        let mempool = Arc::new(SizedMempool::with_capacity(6));

        let messages = vec![Init, RequestTxIds(0, 12, Blocking::Yes)];
        let outcomes = run_messages(mempool, messages).await?;
        assert_outcomes_eq(
            &outcomes,
            &[
                Send(Init),
                Error(MaxOutstandingTxIdsRequested(12, MAX_REQUESTED_TX_IDS)),
            ],
        );
        Ok(())
    }

    #[tokio::test]
    async fn non_blocking_requested_nb_must_be_less_than_protocol_limit() -> anyhow::Result<()> {
        let mempool = Arc::new(SizedMempool::with_capacity(6));

        let messages = vec![Init, RequestTxIds(0, 12, Blocking::No)];
        let outcomes = run_messages(mempool, messages).await?;
        assert_outcomes_eq(
            &outcomes,
            &[
                Send(Init),
                Error(MaxOutstandingTxIdsRequested(12, MAX_REQUESTED_TX_IDS)),
            ],
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_blocking_request_must_be_made_when_all_txs_are_acknowledged() -> anyhow::Result<()> {
        let mempool = Arc::new(SizedMempool::with_capacity(4));
        let txs = create_transactions_in_mempool(mempool.clone(), 4);

        let messages = vec![
            Init,
            request_tx_ids(0, 4, Blocking::Yes),
            request_txs(&txs, &[0, 1]),
            request_tx_ids(2, 4, Blocking::No),
            request_txs(&txs, &[2, 3]),
            request_tx_ids(2, 4, Blocking::No),
        ];
        let outcomes = run_messages(mempool, messages).await?;
        assert_outcomes_eq(
            &outcomes,
            &[
                Send(Init),
                Send(reply_tx_ids(&txs, &[0, 1, 2, 3])),
                Send(reply_txs(&txs, &[0, 1])),
                Send(reply_tx_ids(&txs, &[])),
                Send(reply_txs(&txs, &[2, 3])),
                Error(NonBlockingRequestMadeWhenAllTxsAcknowledged),
            ],
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_non_blocking_request_must_be_made_when_some_txs_are_unacknowledged()
    -> anyhow::Result<()> {
        let mempool = Arc::new(SizedMempool::with_capacity(4));
        let txs = create_transactions_in_mempool(mempool.clone(), 4);

        let messages = vec![
            Init,
            request_tx_ids(0, 4, Blocking::Yes),
            request_txs(&txs, &[0, 1]),
            request_tx_ids(2, 4, Blocking::Yes),
        ];
        let outcomes = run_messages(mempool, messages).await?;
        assert_outcomes_eq(
            &outcomes,
            &[
                Send(Init),
                Send(reply_tx_ids(&txs, &[0, 1, 2, 3])),
                Send(reply_txs(&txs, &[0, 1])),
                Error(BlockingRequestMadeWhenTxsStillUnacknowledged),
            ],
        );
        Ok(())
    }

    #[tokio::test]
    async fn the_responder_cannot_acknowledge_more_than_the_current_unacknowledged_blocking()
    -> anyhow::Result<()> {
        let mempool = Arc::new(SizedMempool::with_capacity(4));
        let txs = create_transactions_in_mempool(mempool.clone(), 4);

        let messages = vec![
            Init,
            request_tx_ids(0, 4, Blocking::Yes),
            request_txs(&txs, &[0, 1]),
            request_tx_ids(2, 4, Blocking::No),
            request_txs(&txs, &[2, 3]),
            request_tx_ids(4, 4, Blocking::Yes),
        ];
        let outcomes = run_messages(mempool, messages).await?;
        assert_outcomes_eq(
            &outcomes,
            &[
                Send(Init),
                Send(reply_tx_ids(&txs, &[0, 1, 2, 3])),
                Send(reply_txs(&txs, &[0, 1])),
                Send(reply_tx_ids(&txs, &[])),
                Send(reply_txs(&txs, &[2, 3])),
                Error(TooManyAcknowledgedTxs(4, 2)),
            ],
        );
        Ok(())
    }

    #[tokio::test]
    async fn the_responder_cannot_acknowledge_more_than_the_current_unacknowledged_non_blocking()
    -> anyhow::Result<()> {
        let mempool = Arc::new(SizedMempool::with_capacity(4));
        let txs = create_transactions_in_mempool(mempool.clone(), 4);

        let messages = vec![
            Init,
            request_tx_ids(0, 4, Blocking::Yes),
            request_txs(&txs, &[0, 1]),
            request_tx_ids(2, 4, Blocking::No),
            request_txs(&txs, &[2, 3]),
            request_tx_ids(4, 4, Blocking::No),
        ];
        let outcomes = run_messages(mempool, messages).await?;
        assert_outcomes_eq(
            &outcomes,
            &[
                Send(Init),
                Send(reply_tx_ids(&txs, &[0, 1, 2, 3])),
                Send(reply_txs(&txs, &[0, 1])),
                Send(reply_tx_ids(&txs, &[])),
                Send(reply_txs(&txs, &[2, 3])),
                Error(TooManyAcknowledgedTxs(4, 2)),
            ],
        );
        Ok(())
    }

    // HELPERS

    async fn run_messages(
        mempool: Arc<dyn TxSubmissionMempool<Tx>>,
        requests: Vec<Message>,
    ) -> anyhow::Result<Vec<Outcome>> {
        let (outcomes, _tx_state, _initiator) = run_messages_state(mempool, requests).await?;
        Ok(outcomes)
    }

    async fn run_messages_state(
        mempool: Arc<dyn TxSubmissionMempool<Tx>>,
        requests: Vec<Message>,
    ) -> anyhow::Result<(Vec<Outcome>, TxSubmissionState, TxSubmissionInitiatorState)> {
        run_messages_state_with(
            TxSubmissionState::Init,
            TxSubmissionInitiatorState::new(),
            mempool,
            requests,
        )
        .await
    }

    async fn run_messages_state_with(
        mut tx_state: TxSubmissionState,
        mut initiator: TxSubmissionInitiatorState,
        mempool: Arc<dyn TxSubmissionMempool<Tx>>,
        requests: Vec<Message>,
    ) -> anyhow::Result<(Vec<Outcome>, TxSubmissionState, TxSubmissionInitiatorState)> {
        let mut outcomes = vec![];
        for r in requests {
            let (new_tx_state, outcome) = initiator.step(mempool.as_ref(), &tx_state, r).await?;
            tx_state = new_tx_state;
            outcomes.push(outcome);
        }
        Ok((outcomes, tx_state, initiator))
    }
}
