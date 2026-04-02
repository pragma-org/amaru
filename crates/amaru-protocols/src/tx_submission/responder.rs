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

use std::{
    collections::{BTreeSet, VecDeque},
    fmt::Display,
    time::Duration,
};

use ProtocolError::*;
use amaru_kernel::Transaction;
use amaru_observability::trace_span;
use amaru_ouroboros::TxSubmissionMempool;
use amaru_ouroboros_traits::{MempoolError, TxId, TxInsertResult, TxOrigin, TxRejectReason};
use pure_stage::{DeserializerGuards, Effects, StageRef, Void};
use tracing::Instrument;

use crate::{
    mempool_effects::MemoryPool,
    mux::MuxMessage,
    protocol::{
        Inputs, Miniprotocol, Outcome, PROTO_N2N_TX_SUB, ProtocolState, Responder, StageState, miniprotocol, outcome,
    },
    tx_submission::{Blocking, Message, ProtocolError, ResponderParams, State},
};

pub const MEMPOOL_INSERT_TIMEOUT: Duration = Duration::from_secs(5);

pub fn register_deserializers() -> DeserializerGuards {
    vec![
        pure_stage::register_data_deserializer::<TxSubmissionResponder>().boxed(),
        pure_stage::register_data_deserializer::<(State, TxSubmissionResponder)>().boxed(),
    ]
}

pub fn responder() -> Miniprotocol<State, TxSubmissionResponder, Responder> {
    miniprotocol(PROTO_N2N_TX_SUB.responder())
}

impl StageState<State, Responder> for TxSubmissionResponder {
    type LocalIn = Void;

    async fn local(
        self,
        _proto: &State,
        input: Self::LocalIn,
        _eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<ResponderAction>, Self)> {
        match input {}
    }

    async fn network(
        mut self,
        _proto: &State,
        input: ResponderResult,
        eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<ResponderAction>, Self)> {
        let message_type = input.message_type().to_string();

        async move {
            let mempool: &dyn TxSubmissionMempool<Transaction> = &MemoryPool::new(eff.clone());

            let action = match input {
                ResponderResult::Init => {
                    tracing::trace!("received Init");
                    self.initialize_state(mempool)
                }
                ResponderResult::ReplyTxIds(tx_ids) => self.process_tx_ids_reply(mempool, tx_ids)?,
                ResponderResult::ReplyTxs(txs) => self.insert_txs(txs, eff).await?,
                ResponderResult::Done => None,
            };
            Ok((action, self))
        }
        .instrument(trace_span!(
            amaru_observability::amaru::protocols::tx_submission::responder::TX_SUBMISSION_RESPONDER_STAGE,
            message_type = message_type
        ))
        .await
    }

    fn muxer(&self) -> &StageRef<MuxMessage> {
        &self.muxer
    }
}

impl ProtocolState<Responder> for State {
    type WireMsg = Message;
    type Action = ResponderAction;
    type Out = ResponderResult;
    type Error = ProtocolError;

    fn init(&self) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out, Self::Error>, Self)> {
        // Responder waits for Init message, doesn't send anything on init
        Ok((outcome().want_next(), *self))
    }

    fn network(&self, input: Self::WireMsg) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out, Self::Error>, Self)> {
        let _span = trace_span!(
            amaru_observability::amaru::protocols::tx_submission::responder::TX_SUBMISSION_RESPONDER_PROTOCOL,
            message_type = input.message_type().to_string()
        );
        let _guard = _span.enter();
        Ok(match (self, input) {
            (State::Init, Message::Init) => (outcome().result(ResponderResult::Init), State::Idle),
            (State::TxIdsBlocking | State::TxIdsNonBlocking, Message::ReplyTxIds(tx_ids)) => {
                (outcome().result(ResponderResult::ReplyTxIds(tx_ids)), State::Idle)
            }
            (State::Txs, Message::ReplyTxs(txs)) => (outcome().result(ResponderResult::ReplyTxs(txs)), State::Idle),
            (State::TxIdsBlocking, Message::Done) => (outcome().result(ResponderResult::Done), State::Done),
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }

    fn local(&self, input: Self::Action) -> anyhow::Result<(Outcome<Self::WireMsg, Void, Self::Error>, Self)> {
        Ok(match (self, input) {
            (State::Idle, ResponderAction::SendRequestTxIds { ack, req, blocking }) => match blocking {
                Blocking::Yes => {
                    (outcome().send(Message::RequestTxIdsBlocking(ack, req)).want_next(), State::TxIdsBlocking)
                }
                Blocking::No => {
                    (outcome().send(Message::RequestTxIdsNonBlocking(ack, req)).want_next(), State::TxIdsNonBlocking)
                }
            },
            (State::Idle, ResponderAction::SendRequestTxs(tx_ids)) => {
                (outcome().send(Message::RequestTxs(tx_ids)).want_next(), State::Txs)
            }
            (_, ResponderAction::Error(e)) => (outcome().terminate_with(e), State::Done),
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }
}

/// Result from protocol state when network message is received
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ResponderResult {
    Init,
    ReplyTxIds(Vec<(TxId, u32)>),
    ReplyTxs(Vec<Transaction>),
    Done,
}

impl ResponderResult {
    pub fn message_type(&self) -> &str {
        match self {
            ResponderResult::Init => "Init",
            ResponderResult::ReplyTxIds(_) => "ReplyTxIds",
            ResponderResult::ReplyTxs(_) => "ReplyTxs",
            ResponderResult::Done => "Done",
        }
    }
}

impl Display for ResponderResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResponderResult::Init => write!(f, "Init"),
            ResponderResult::ReplyTxIds(tx_ids) => {
                write!(f, "ReplyTxIds(len: {})", tx_ids.len())
            }
            ResponderResult::ReplyTxs(txs) => write!(f, "ReplyTxs(len: {})", txs.len()),
            ResponderResult::Done => write!(f, "Done"),
        }
    }
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TxSubmissionResponder {
    /// Responder parameters: batch sizes, window sizes, etc.
    params: ResponderParams,
    /// All tx_ids advertised but not yet acked (and their size).
    window: VecDeque<(TxId, u32)>,
    /// Tx ids we want to fetch but haven't yet requested.
    pending_fetch: VecDeque<TxId>,
    /// Then as a set for quick lookup when processing received ids.
    /// This is kept in sync with `inflight_fetch_queue`. When we receive a tx body,
    /// we pop it from the front of the queue and remove it from the set.
    inflight_fetch_set: BTreeSet<TxId>,
    /// The origin of the transactions we are fetching.
    origin: TxOrigin,
    muxer: StageRef<MuxMessage>,
    /// Reference to the mempool stage for batch transaction insertion.
    mempool_stage: StageRef<MempoolMsg>,
}

impl TxSubmissionResponder {
    pub fn new(
        muxer: StageRef<MuxMessage>,
        params: ResponderParams,
        origin: TxOrigin,
        mempool_stage: StageRef<MempoolMsg>,
    ) -> (State, Self) {
        (
            State::Init,
            Self {
                params,
                window: VecDeque::new(),
                pending_fetch: VecDeque::new(),
                inflight_fetch_set: BTreeSet::new(),
                origin,
                muxer,
                mempool_stage,
            },
        )
    }

    fn initialize_state(&mut self, mempool: &dyn TxSubmissionMempool<Transaction>) -> Option<ResponderAction> {
        let (ack, req, blocking) = self.request_tx_ids(mempool);
        Some(ResponderAction::SendRequestTxIds { ack, req, blocking })
    }

    fn process_tx_ids_reply(
        &mut self,
        mempool: &dyn TxSubmissionMempool<Transaction>,
        tx_ids: Vec<(TxId, u32)>,
    ) -> anyhow::Result<Option<ResponderAction>> {
        if self.window.len() + tx_ids.len() > self.params.max_window.into() {
            return protocol_error(TooManyTxIdsReceived(
                tx_ids.len(),
                self.window.len(),
                self.params.max_window.into(),
            ));
        }
        self.received_tx_ids(mempool, tx_ids);

        let txs = self.txs_to_request();
        if txs.is_empty() {
            let (ack, req, blocking) = self.request_tx_ids(mempool);
            Ok(Some(ResponderAction::SendRequestTxIds { ack, req, blocking }))
        } else {
            Ok(Some(ResponderAction::SendRequestTxs(txs)))
        }
    }

    /// Prepare a request for tx ids, acknowledging already processed ones
    /// and requesting as many as fit in the window.
    #[allow(clippy::expect_used)]
    fn request_tx_ids(&mut self, mempool: &dyn TxSubmissionMempool<Transaction>) -> (u16, u16, Blocking) {
        // Acknowledge everything we’ve already processed.
        let mut ack = 0_u16;

        while let Some((tx_id, _size)) = self.window.front() {
            let already_in_mempool = mempool.contains(tx_id);
            if already_in_mempool {
                // pop from window and ack it
                if self.window.pop_front().is_some() {
                    ack = ack.checked_add(1).expect("ack overflow: protocol invariant violated");
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
        let blocking = if self.window.is_empty() { Blocking::Yes } else { Blocking::No };
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
                self.inflight_fetch_set.insert(id);
                tx_ids.push(id);
            } else {
                break;
            }
        }

        tx_ids
    }

    /// Validate, batch-insert via the mempool stage, log results, then request next tx ids.
    ///
    /// All writes to the mempool are sent to the mempool stage so that access is serialised.
    /// Hard errors (mempool stage unavailable or timed out) terminate the connection.
    async fn insert_txs(
        &mut self,
        txs: Vec<Transaction>,
        eff: &Effects<Inputs<Void>>,
    ) -> anyhow::Result<Option<ResponderAction>> {
        // 'action' in that case represents an error message that we want to return to the initiator.
        if let Some(action) = self.validate_received_txs(&txs)? {
            return Ok(Some(action));
        }

        let tx_ids: Vec<TxId> = txs.iter().map(TxId::from).collect();
        for tx_id in &tx_ids {
            self.inflight_fetch_set.remove(tx_id);
        }

        let origin = self.origin.clone();
        match eff
            .call(&self.mempool_stage, MEMPOOL_INSERT_TIMEOUT, move |caller| MempoolMsg::InsertBatch {
                txs,
                origin: origin.clone(),
                caller,
            })
            .await
        {
            None => return protocol_error(MempoolBatchInsertFailedTimedout),
            Some(Err(error)) => return protocol_error(MempoolInsertFailed(error.tx_id, error.error)),
            Some(Ok(results)) => {
                for result in results {
                    log_insert_result(&result);
                }
            }
        }

        let mempool = MemoryPool::new(eff.clone());
        let (ack, req, blocking) = self.request_tx_ids(&mempool);
        Ok(Some(ResponderAction::SendRequestTxIds { ack, req, blocking }))
    }

    /// Check:
    ///  - That the number of received transactions does not exceed the batch size
    ///  - That there are no duplicate transactions
    ///  - That the received transactions correspond to requested transactions
    ///
    fn validate_received_txs(&self, txs: &[Transaction]) -> anyhow::Result<Option<ResponderAction>> {
        if txs.len() > self.params.fetch_batch.into() {
            return protocol_error(ReceivedTxsExceedsBatchSize(txs.len(), self.params.fetch_batch.into()));
        }

        let tx_ids_set = txs.iter().map(TxId::from).collect::<BTreeSet<_>>();
        if tx_ids_set.len() != txs.len() {
            let tx_ids = txs.iter().map(TxId::from).collect::<Vec<_>>();
            return protocol_error(DuplicateTxIds(tx_ids));
        }

        let not_in_flight =
            tx_ids_set.iter().filter(|tx_id| !self.inflight_fetch_set.contains(*tx_id)).cloned().collect::<Vec<_>>();
        if !not_in_flight.is_empty() {
            return protocol_error(SomeReceivedTxsNotInFlight(not_in_flight));
        }

        Ok(None)
    }
}

fn log_insert_result(result: &TxInsertResult) {
    match result {
        TxInsertResult::Accepted { tx_id, .. } => {
            tracing::debug!("insert transaction {} into the mempool", tx_id);
        }
        TxInsertResult::Rejected { tx_id, reason: TxRejectReason::Invalid(error) } => {
            tracing::warn!("received invalid transaction {}: {}", tx_id, error);
        }
        TxInsertResult::Rejected { tx_id, reason: TxRejectReason::MempoolFull } => {
            tracing::warn!("mempool full, dropping transaction {}", tx_id);
        }
        TxInsertResult::Rejected { tx_id, reason: TxRejectReason::Duplicate } => {
            tracing::debug!("duplicate transaction {}, skipping", tx_id);
        }
    }
}

fn protocol_error(error: ProtocolError) -> anyhow::Result<Option<ResponderAction>> {
    tracing::warn!("protocol error: {error}");
    Ok(Some(ResponderAction::Error(error)))
}

impl AsRef<StageRef<MuxMessage>> for TxSubmissionResponder {
    fn as_ref(&self) -> &StageRef<MuxMessage> {
        &self.muxer
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ResponderAction {
    SendRequestTxIds { ack: u16, req: u16, blocking: Blocking },
    SendRequestTxs(Vec<TxId>),
    Error(ProtocolError),
}

impl Display for ResponderAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResponderAction::SendRequestTxIds { ack, req, blocking } => {
                write!(f, "SendRequestTxIds(ack: {}, req: {}, blocking: {:?})", ack, req, blocking)
            }
            ResponderAction::SendRequestTxs(tx_ids) => {
                write!(f, "SendRequestTxs(tx_ids: {:?})", tx_ids)
            }
            ResponderAction::Error(err) => write!(f, "Error({})", err),
        }
    }
}

/// Messages accepted by the mempool stage.
///
/// `Insert` is used for single-transaction submission (e.g. the HTTP Submit API).
///
/// `InsertBatch` is used for bulk insertion from the TX submission protocol,
/// where transactions arrive in batches and a single round-trip is preferable.
///
/// The response to `InsertBatch` contains one `TxInsertResult` per input transaction,
/// in the same order.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MempoolInsertError {
    pub tx_id: TxId,
    pub error: MempoolError,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum MempoolMsg {
    Insert {
        tx: Box<Transaction>,
        origin: TxOrigin,
        caller: StageRef<Result<TxInsertResult, MempoolInsertError>>,
    },
    InsertBatch {
        txs: Vec<Transaction>,
        origin: TxOrigin,
        caller: StageRef<Result<Vec<TxInsertResult>, MempoolInsertError>>,
    },
}

#[cfg(test)]
mod tests {

    use std::{collections::BTreeMap, pin::Pin, sync::Arc};

    use amaru_kernel::Transaction;
    use amaru_mempool::strategies::InMemoryMempool;
    use amaru_ouroboros_traits::{MempoolError, MempoolSeqNo, TxInsertResult};

    use super::*;
    use crate::tx_submission::{assert_actions_eq, tests::create_transactions};

    #[tokio::test]
    async fn test_responder() -> anyhow::Result<()> {
        let txs = create_transactions(6);

        // Create a mempool with no initial transactions
        // since we are going to fetch them from the initiator
        let mempool = Arc::new(InMemoryMempool::default());

        // Send replies from the initiator as if they were replies to previous requests from the responder
        let results = vec![
            init(),
            reply_tx_ids(&txs, &[0, 1, 2]),
            reply_txs(&txs, &[0, 1]),
            reply_tx_ids(&txs, &[3, 4, 5]),
            reply_txs(&txs, &[2, 3]),
            reply_tx_ids(&txs, &[]),
            reply_txs(&txs, &[4, 5]),
            done(),
        ];

        let actions = run_stage(mempool.clone(), results).await?;

        assert_actions_eq(
            &actions,
            &[
                request_tx_ids(0, 10, Blocking::Yes),
                request_txs(&txs, &[0, 1]),
                request_tx_ids(2, 9, Blocking::No),
                request_txs(&txs, &[2, 3]),
                request_tx_ids(2, 8, Blocking::No),
                request_txs(&txs, &[4, 5]),
                request_tx_ids(2, 10, Blocking::Yes),
            ],
        );
        Ok(())
    }

    #[tokio::test]
    async fn the_returned_tx_ids_should_respect_the_window_size() -> anyhow::Result<()> {
        let txs = create_transactions(11);
        let mempool = Arc::new(InMemoryMempool::default());

        let results = vec![init(), reply_tx_ids(&txs, &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10])];

        let actions = run_stage(mempool.clone(), results).await?;
        assert_actions_eq(
            &actions,
            &[request_tx_ids(0, 10, Blocking::Yes), error_action(TooManyTxIdsReceived(11, 0, 10))],
        );
        Ok(())
    }

    #[tokio::test]
    async fn the_returned_txs_should_respect_the_batch_size() -> anyhow::Result<()> {
        let txs = create_transactions(6);
        let mempool = Arc::new(InMemoryMempool::default());

        let results = vec![
            init(),
            reply_tx_ids(&txs, &[0, 1, 2]),
            reply_txs(&txs, &[0]),
            reply_tx_ids(&txs, &[]),
            reply_txs(&txs, &[1, 2, 3]),
        ];

        let outcomes = run_stage(mempool.clone(), results).await?;
        assert_actions_eq(
            &outcomes,
            &[
                request_tx_ids(0, 10, Blocking::Yes),
                request_txs(&txs, &[0, 1]),
                request_tx_ids(1, 8, Blocking::No),
                request_txs(&txs, &[2]),
                error_action(ReceivedTxsExceedsBatchSize(3, 2)),
            ],
        );
        Ok(())
    }

    #[tokio::test]
    async fn the_returned_txs_be_a_subset_of_the_inflight_txs() -> anyhow::Result<()> {
        let txs = create_transactions(6);
        let mempool = Arc::new(InMemoryMempool::default());

        let results = vec![
            init(),
            reply_tx_ids(&txs, &[0, 1, 2]),
            reply_txs(&txs, &[0]),
            reply_tx_ids(&txs, &[]),
            reply_txs(&txs, &[1, 3]),
        ];

        let actions = run_stage(mempool.clone(), results).await?;
        assert_actions_eq(
            &actions,
            &[
                request_tx_ids(0, 10, Blocking::Yes),
                request_txs(&txs, &[0, 1]),
                request_tx_ids(1, 8, Blocking::No),
                request_txs(&txs, &[2]),
                error_action(SomeReceivedTxsNotInFlight(vec![TxId::from(&txs[3])])),
            ],
        );
        Ok(())
    }

    #[tokio::test]
    async fn fatal_mempool_errors_terminate_the_protocol() -> anyhow::Result<()> {
        let txs = create_transactions(2);
        let mempool = Arc::new(FailingInsertMempool::new("database unavailable"));

        let actions = run_stage(mempool, vec![init(), reply_tx_ids(&txs, &[0, 1]), reply_txs(&txs, &[0, 1])]).await?;

        assert_actions_eq(
            &actions,
            &[
                request_tx_ids(0, 10, Blocking::Yes),
                request_txs(&txs, &[0, 1]),
                error_action(MempoolInsertFailed(TxId::from(&txs[0]), MempoolError::new("database unavailable"))),
            ],
        );
        Ok(())
    }

    #[tokio::test]
    async fn rejected_transactions_at_the_front_of_the_window_are_acknowledged() -> anyhow::Result<()> {
        let txs = create_transactions(3);
        let mempool = Arc::new(MockMempool::new(vec![
            TxInsertResult::rejected(
                TxId::from(&txs[0]),
                TxRejectReason::Invalid(TransactionValidationError::from(anyhow::anyhow!("invalid for test"))),
            ),
            TxInsertResult::accepted(TxId::from(&txs[1]), MempoolSeqNo(1)),
        ]));

        let actions =
            run_stage(mempool, vec![init(), reply_tx_ids(&txs, &[0, 1, 2]), reply_txs(&txs, &[0, 1])]).await?;

        assert_actions_eq(
            &actions,
            &[request_tx_ids(0, 10, Blocking::Yes), request_txs(&txs, &[0, 1]), request_tx_ids(2, 9, Blocking::No)],
        );
        Ok(())
    }
    #[test]
    fn test_responder_protocol() {
        crate::tx_submission::spec::<Responder>().check(State::Init, |msg| match msg {
            Message::RequestTxIdsBlocking(ack, req) => {
                Some(ResponderAction::SendRequestTxIds { ack: *ack, req: *req, blocking: Blocking::Yes })
            }
            Message::RequestTxIdsNonBlocking(ack, req) => {
                Some(ResponderAction::SendRequestTxIds { ack: *ack, req: *req, blocking: Blocking::No })
            }
            Message::RequestTxs(txs) => Some(ResponderAction::SendRequestTxs(txs.clone())),
            Message::ReplyTxs(_) | Message::ReplyTxIds(_) | Message::Init | Message::Done => None,
        });
    }

    // HELPERS

    /// Run the responder stage, given a list of ResponderResults as inputs, and return the list of
    /// ResponderActions produced as output.
    async fn run_stage(
        mempool: Arc<dyn TxSubmissionMempool<Transaction>>,
        results: Vec<ResponderResult>,
    ) -> anyhow::Result<Vec<ResponderAction>> {
        Ok(run_stage_and_return_state(mempool, results).await?.0)
    }

    /// Run the responder stage, given a list of ResponderResults as inputs, and return the list of
    /// ResponderActions produced as output, plus the responder itself
    async fn run_stage_and_return_state(
        mempool: Arc<dyn TxSubmissionMempool<Transaction>>,
        results: Vec<ResponderResult>,
    ) -> anyhow::Result<(Vec<ResponderAction>, TxSubmissionResponder)> {
        run_stage_and_return_state_with(
            TxSubmissionResponder::new(
                StageRef::named_for_tests("muxer"),
                ResponderParams::default(),
                TxOrigin::Local,
                StageRef::blackhole(),
            )
            .1,
            mempool,
            results,
        )
        .await
    }

    /// Run the responder stage with a list of ResponderResults as input, and return the list of
    /// ResponderActions produced as output, along with the final state of the responder.
    async fn run_stage_and_return_state_with(
        mut responder: TxSubmissionResponder,
        mempool: Arc<dyn TxSubmissionMempool<Transaction>>,
        results: Vec<ResponderResult>,
    ) -> anyhow::Result<(Vec<ResponderAction>, TxSubmissionResponder)> {
        let mut actions = vec![];
        for r in results {
            let action = match r {
                ResponderResult::Init => responder.initialize_state(mempool.as_ref()),
                ResponderResult::ReplyTxIds(tx_ids) => responder.process_tx_ids_reply(mempool.as_ref(), tx_ids)?,
                ResponderResult::ReplyTxs(txs) => {
                    // insert transactions directly into the mempool, without sending a message to the mempool stage.
                    if let Some(action) = responder.validate_received_txs(&txs)? {
                        Some(action)
                    } else {
                        let origin = responder.origin.clone();
                        let mut error = None;
                        for tx in txs {
                            let requested_id = TxId::from(&tx);
                            responder.inflight_fetch_set.remove(&requested_id);
                            if let Err(e) = mempool.insert(tx, origin.clone()) {
                                error = Some(protocol_error(MempoolInsertFailed(requested_id, e))?);
                                break;
                            }
                        }
                        if let Some(error) = error {
                            error
                        } else {
                            let (ack, req, blocking) = responder.request_tx_ids(mempool.as_ref());
                            Some(ResponderAction::SendRequestTxIds { ack, req, blocking })
                        }
                    }
                }
                ResponderResult::Done => None,
            };
            if let Some(action) = action {
                actions.push(action)
            };
        }
        Ok((actions, responder))
    }

    fn init() -> ResponderResult {
        ResponderResult::Init
    }

    fn done() -> ResponderResult {
        ResponderResult::Done
    }

    fn reply_tx_ids(txs: &[Transaction], ids: &[usize]) -> ResponderResult {
        ResponderResult::ReplyTxIds(ids.iter().map(|id| (TxId::from(&txs[*id]), 50)).collect())
    }

    fn reply_txs(txs: &[Transaction], ids: &[usize]) -> ResponderResult {
        ResponderResult::ReplyTxs(ids.iter().map(|id| txs[*id].clone()).collect())
    }

    fn request_tx_ids(ack: u16, req: u16, blocking: Blocking) -> ResponderAction {
        ResponderAction::SendRequestTxIds { ack, req, blocking }
    }

    fn request_txs(txs: &[Transaction], ids: &[usize]) -> ResponderAction {
        ResponderAction::SendRequestTxs(ids.iter().map(|id| TxId::from(&txs[*id])).collect())
    }

    fn error_action(error: ProtocolError) -> ResponderAction {
        ResponderAction::Error(error)
    }

    ///  A mempool that is completely failing to insert anything
    struct FailingInsertMempool {
        message: &'static str,
    }

    impl FailingInsertMempool {
        fn new(message: &'static str) -> Self {
            Self { message }
        }
    }

    impl TxSubmissionMempool<Transaction> for FailingInsertMempool {
        fn insert(&self, _tx: Transaction, _tx_origin: TxOrigin) -> Result<TxInsertResult, MempoolError> {
            Err(MempoolError::new(self.message))
        }

        fn get_tx(&self, _tx_id: &TxId) -> Option<Transaction> {
            None
        }

        fn tx_ids_since(&self, _from_seq: MempoolSeqNo, _limit: u16) -> Vec<(TxId, u32, MempoolSeqNo)> {
            vec![]
        }

        fn wait_for_at_least(&self, _seq_no: MempoolSeqNo) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
            Box::pin(async { false })
        }

        fn get_txs_for_ids(&self, _ids: &[TxId]) -> Vec<Transaction> {
            vec![]
        }

        fn last_seq_no(&self) -> MempoolSeqNo {
            MempoolSeqNo(0)
        }
    }

    struct MockMempool {
        results: std::sync::Mutex<BTreeMap<TxId, TxInsertResult>>,
    }

    impl MockMempool {
        fn new(results: Vec<TxInsertResult>) -> Self {
            Self { results: std::sync::Mutex::new(results.into_iter().map(|result| (*result.tx_id(), result)).collect()) }
        }
    }

    impl TxSubmissionMempool<Transaction> for MockMempool {
        fn insert(&self, tx: Transaction, _tx_origin: TxOrigin) -> Result<TxInsertResult, MempoolError> {
            let tx_id = TxId::from(&tx);
            Ok(self
                .results
                .lock()
                .expect("mock mempool results mutex poisoned")
                .remove(&tx_id)
                .expect("missing insert result for mock mempool test"))
        }

        fn get_tx(&self, _tx_id: &TxId) -> Option<Transaction> {
            None
        }

        fn contains(&self, tx_id: &TxId) -> bool {
            let _ = tx_id;
            false
        }

        fn tx_ids_since(&self, _from_seq: MempoolSeqNo, _limit: u16) -> Vec<(TxId, u32, MempoolSeqNo)> {
            vec![]
        }

        fn wait_for_at_least(&self, _seq_no: MempoolSeqNo) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
            Box::pin(async { false })
        }

        fn get_txs_for_ids(&self, _ids: &[TxId]) -> Vec<Transaction> {
            vec![]
        }

        fn last_seq_no(&self) -> MempoolSeqNo {
            MempoolSeqNo(0)
        }
    }
}
