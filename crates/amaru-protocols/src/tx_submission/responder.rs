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
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt::Display,
};

use amaru_kernel::{to_cbor, Transaction};
use amaru_observability::trace_span;
use amaru_ouroboros::{
    MempoolInsertError, MempoolMsg, MempoolSeqNo, TxId, TxInsertResult, TxOrigin, TxRejectReason, TxSubmissionMempool,
};
use amaru_ouroboros_traits::{TxId, TxInsertResult, TxOrigin, TxRejectReason};
use pure_stage::{DeserializerGuards, Effects, ScheduleId, StageRef, Void};
use tracing::Instrument;
use ProtocolError::*;

use crate::{
    mempool_effects::{AsyncMempool, MemoryPool},
    mux::MuxMessage,
    protocol::{
        miniprotocol, outcome, Inputs, Miniprotocol, Outcome, ProtocolState, Responder, StageState, PROTO_N2N_TX_SUB,
    },
    tx_submission::{Blocking, Message, ProtocolError, ResponderParams, State},
};

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
    type LocalIn = ResponderLocalIn;

    async fn local(
        mut self,
        proto: &State,
        input: Self::LocalIn,
        eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<ResponderAction>, Self)> {
        let action = match input {
            ResponderLocalIn::CheckMempoolSize => {
                let mempool: &dyn TxSubmissionMempool<Transaction> = &MemoryPool::new(eff.clone());
                self.back_pressure_scheduled = false;
                self.recheck_back_pressure(mempool, eff).await
            }
            ResponderLocalIn::InflightTimeout(fetch_id) => self.handle_inflight_timeout(fetch_id, proto),
        };
        Ok((action, self))
    }

    async fn network(
        mut self,
        _proto: &State,
        input: ResponderResult,
        eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<ResponderAction>, Self)> {
        let message_type = input.message_type().to_string();

        async move {
            let mempool = MemoryPool::new(eff.clone());

            let action = match input {
                ResponderResult::Init => {
                    tracing::trace!("received Init");
                    self.initialize_state(&mempool).await
                }
                ResponderResult::ReplyTxIds(tx_ids) => match self.process_tx_ids_reply(mempool, tx_ids).await? {
                    FetchOutcome::Action(action) => {
                        self.schedule_inflight_timeout(&action, eff).await;
                        Some(action)
                    }
                    FetchOutcome::AwaitingCapacity => {
                        self.schedule_back_pressure_recheck(eff).await;
                        None
                    }
                },
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

/// Self-message variants delivered to the responder via `eff.schedule_after`.
#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ResponderLocalIn {
    /// Triggered after `params.back_pressure_recheck_interval` while the mempool was too full to
    /// accept the next pending tx. On firing, the responder re-attempts to drain `pending_fetch`
    /// and resumes the protocol if there is new mempool capacity.
    CheckMempoolSize,
    /// Triggered `params.inflight_fetch_timeout` after a `RequestTxs` was sent. The carried `u64` is the
    /// `fetch_generation` value at the time the timer was scheduled; on fire we ignore any timer
    /// whose generation no longer matches `fetch_generation` (a `ReplyTxs` has since been
    /// received and a new batch may already be in flight). One timer per `RequestTxs` round,
    /// not per tx_id — this matches Cardano Haskell's per-state codec time limit.
    InflightTimeout(u64),
}

/// Result of `process_tx_ids_reply`. The synchronous decision separates "what to send to the peer"
/// from "should we schedule a back-pressure recheck", so test harnesses don't need an `Effects`
/// instance to drive the protocol.
#[derive(Debug)]
pub enum FetchOutcome {
    /// Send the carried action to the peer.
    Action(ResponderAction),
    /// Mempool is full and we have unfetched pending entries; caller should call
    /// `schedule_back_pressure_recheck` and stay quiet until the recheck fires.
    AwaitingCapacity,
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
    /// Fetched tx ids that were fully processed, even if they were rejected.
    processed_fetch_set: BTreeSet<TxId>,
    /// Tx ids we want to fetch but haven't yet requested.
    pending_fetch: VecDeque<TxId>,
    /// Tx ids we have requested bodies for, mapped to the size advertised by the peer in
    /// `ReplyTxIds`. Used to verify on `ReplyTxs` that the body's CBOR size matches what was
    /// advertised.
    inflight_fetch: BTreeMap<TxId, u32>,
    /// Fetch counter incremented each time we send `RequestTxs`. It is use to schedule `InflightTimeout`
    /// messages, that will terminate the connection if no transactions have been received for the current
    /// fetch_id.
    fetch_id: u64,
    /// Used to avoid scheduling multiple back-pressure rechecks concurrently.
    back_pressure_scheduled: bool,
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
                processed_fetch_set: BTreeSet::new(),
                pending_fetch: VecDeque::new(),
                inflight_fetch: BTreeMap::new(),
                fetch_id: 0,
                back_pressure_scheduled: false,
                origin,
                muxer,
                mempool_stage,
            },
        )
    }

    async fn initialize_state(&mut self, mempool: &dyn AsyncMempool) -> Option<ResponderAction> {
        let (ack, req, blocking) = self.request_tx_ids(mempool).await;
        Some(ResponderAction::SendRequestTxIds { ack, req, blocking })
    }

    async fn process_tx_ids_reply(
        &mut self,
        mempool: &dyn AsyncMempool,
        tx_ids: Vec<(TxId, u32)>,
    ) -> anyhow::Result<FetchOutcome> {
        if self.window.len() + tx_ids.len() > self.params.max_window.get().into() {
            return protocol_error_outcome(TooManyTxIdsReceived(
                tx_ids.len(),
                self.window.len(),
                self.params.max_window.get().into(),
            ));
        }
        self.received_tx_ids(mempool, tx_ids).await;

        let txs = self.txs_to_request();
        if !txs.is_empty() {
            return Ok(FetchOutcome::Action(ResponderAction::SendRequestTxs(txs)));
        }

        // No fetchable txs right now. If we still have entries in pending_fetch we couldn't drain,
        // it must be because each remaining advertised tx_id has a size that would push the mempool
        // over its configured maximum byte size.
        // In that case, we schedule a back-pressure recheck.
        if !self.pending_fetch.is_empty() {
            return Ok(FetchOutcome::AwaitingCapacity);
        }

        let (ack, req, blocking) = self.request_tx_ids(mempool).await;
        Ok(FetchOutcome::Action(ResponderAction::SendRequestTxIds { ack, req, blocking }))
    }

    #[cfg(test)]
    async fn process_txs_reply(
        &mut self,
        mempool: &dyn AsyncMempool,
        txs: Vec<Transaction>,
        origin: TxOrigin,
    ) -> anyhow::Result<Option<ResponderAction>> {
        if txs.len() > self.params.fetch_batch.into() {
            return protocol_error(ReceivedTxsExceedsBatchSize(txs.len(), self.params.fetch_batch.into()));
        }

        // check for duplicate tx ids
        let tx_ids = txs.iter().map(TxId::from).collect::<BTreeSet<_>>();
        if tx_ids.len() != txs.len() {
            // return the full list of tx ids including duplicates
            let tx_ids = txs.iter().map(TxId::from).collect::<Vec<_>>();
            return protocol_error(DuplicateTxIds(tx_ids));
        }

        // check that all received tx ids were in-flight
        let not_in_flight =
            tx_ids.iter().filter(|tx_id| !self.inflight_fetch_set.contains(tx_id)).cloned().collect::<Vec<_>>();
        if !not_in_flight.is_empty() {
            return protocol_error(SomeReceivedTxsNotInFlight(not_in_flight));
        }

        if let Some(action) = self.received_txs(mempool, txs, origin).await? {
            return Ok(Some(action));
        }
        let (ack, req, blocking) = self.request_tx_ids(mempool).await;
        Ok(Some(ResponderAction::SendRequestTxIds { ack, req, blocking }))
    }

    /// Prepare a request for tx ids, acknowledging already processed ones
    /// and requesting as many as fit in the window.
    #[allow(clippy::expect_used)]
    async fn request_tx_ids(&mut self, mempool: &dyn AsyncMempool) -> (u16, u16, Blocking) {
        // Acknowledge everything we’ve already processed.
        let mut ack = 0_u16;

        while let Some((tx_id, _size)) = self.window.front() {
            let already_processed = self.processed_fetch_set.remove(tx_id);
            let already_in_mempool = mempool.contains(*tx_id).await;
            if already_processed || already_in_mempool {
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
            .get()
            .checked_sub(self.window.len() as u16)
            .expect("req underflow: protocol invariant violated");

        // We need to block if there are no more outstanding tx ids.
        let blocking = if self.window.is_empty() { Blocking::Yes } else { Blocking::No };
        (ack, req, blocking)
    }

    /// Register received tx ids, adding them to the window and to the pending fetch list
    /// if they are not already in the mempool.
    async fn received_tx_ids(&mut self, mempool: &dyn AsyncMempool, tx_ids: Vec<(TxId, u32)>) {
        for (tx_id, size) in tx_ids {
            // We add the tx id to the window to acknowledge it on the next round.
            self.window.push_back((tx_id, size));

            // We only add to pending fetch if we haven't received it yet in the mempool.
            if !mempool.contains(tx_id).await {
                self.pending_fetch.push_back(tx_id);
            }
        }
    }

    /// Prepare a batch of tx ids to fetch. Greedy under two byte limits:
    /// - the mempool's `is_near_capacity` (back-pressure on insertion);
    /// - `params.fetch_batch_bytes` (per-`RequestTxs` byte budget, mirrors Haskell V2's
    ///   `txsSizeInflightPerPeer`).
    ///
    /// Always serves at least one tx so that an unusually large advertisement doesn't starve
    /// the queue. Remaining `pending_fetch` entries are revisited once capacity returns.
    fn txs_to_request(&mut self, mempool: &dyn TxSubmissionMempool<Transaction>) -> Vec<TxId> {
        let mut tx_ids = Vec::new();
        let mut reserved: u64 = 0;
        let budget = self.params.fetch_batch_bytes.get();

        while let Some(&next_id) = self.pending_fetch.front() {
            let advertised_size = self.window.iter().find(|(id, _)| *id == next_id).map(|(_, sz)| *sz).unwrap_or(0);
            let next_total = reserved.saturating_add(advertised_size as u64);

            // Stop if the mempool can't accept this tx — back-pressure.
            if mempool.is_near_capacity(next_total) {
                break;
            }
            // Stop if including this tx would exceed the per-batch byte budget — but always
            // serve at least one to avoid starving on a single oversized advertisement.
            if !tx_ids.is_empty() && next_total > budget {
                break;
            }

            self.pending_fetch.pop_front();
            self.inflight_fetch.insert(next_id, advertised_size);
            tx_ids.push(next_id);
            reserved = next_total;
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
        eff: &Effects<Inputs<ResponderLocalIn>>,
    ) -> anyhow::Result<Option<ResponderAction>> {
        if let Some(action) = self.validate_received_txs(&txs)? {
            return Ok(Some(action));
        }

        let tx_ids: Vec<TxId> = txs.iter().map(TxId::from).collect();
        for tx_id in &tx_ids {
            self.inflight_fetch.remove(tx_id);
        }

        let origin = self.origin.clone();
        match eff
            .call(&self.mempool_stage, self.params.mempool_insert_timeout.as_duration(), move |caller| {
                MempoolMsg::InsertBatch { txs, origin: origin.clone(), caller }
            })
            .await
        {
            None => return protocol_error(MempoolBatchInsertFailedTimedout),
            Some(Err(error)) => return protocol_error(MempoolInsertFailed(error.tx_id, error.error)),
            Some(Ok(results)) => {
                self.record_processed_results(&results);
                for result in &results {
                    log_insert_result(result);
                }
            }
        }

        let mempool = MemoryPool::new(eff.clone());
        let (ack, req, blocking) = self.request_tx_ids(&mempool).await;
        Ok(Some(ResponderAction::SendRequestTxIds { ack, req, blocking }))
    }

    /// Process received txs, validating and inserting them into the mempool.
    #[cfg(test)]
    async fn received_txs(
        &mut self,
        mempool: &dyn AsyncMempool,
        txs: Vec<Transaction>,
        origin: TxOrigin,
    ) -> anyhow::Result<Option<ResponderAction>> {
        let mut results = Vec::with_capacity(txs.len());
        for tx in txs {
            let requested_id = TxId::from(&tx);
            self.inflight_fetch_set.remove(&requested_id);
            match mempool.insert(tx, origin.clone()).await {
                Ok(result) => {
                    log_insert_result(&result);
                    results.push(result);
                }
                Err(error) => return protocol_error(MempoolInsertFailed(requested_id, error)),
            }
        }

        self.record_processed_results(&results);
        Ok(None)
    }

    /// Check:
    ///  - That there are no duplicate transactions in the batch
    ///  - That every received tx body corresponds to a tx_id we requested (`inflight_fetch`)
    ///  - That each body's CBOR size matches what the peer advertised
    ///
    /// Over-response (peer sends more bodies than we asked for) is caught implicitly: the extra
    /// body's tx_id won't be in `inflight_fetch`, and `SomeReceivedTxsNotInFlight` fires.
    fn validate_received_txs(&self, txs: &[Transaction]) -> anyhow::Result<Option<ResponderAction>> {
        let tx_ids_set = txs.iter().map(TxId::from).collect::<BTreeSet<_>>();
        if tx_ids_set.len() != txs.len() {
            let tx_ids = txs.iter().map(TxId::from).collect::<Vec<_>>();
            return protocol_error(DuplicateTxIds(tx_ids));
        }

        let not_in_flight =
            tx_ids_set.iter().filter(|tx_id| !self.inflight_fetch.contains_key(*tx_id)).cloned().collect::<Vec<_>>();
        if !not_in_flight.is_empty() {
            return protocol_error(SomeReceivedTxsNotInFlight(not_in_flight));
        }

        // Verify that each body's CBOR size matches what the peer advertised in `ReplyTxIds`.
        // A mismatch is a protocol violation so we treat it as a fatal error and disconnect.
        for tx in txs {
            let tx_id = TxId::from(tx);
            let advertised = self.inflight_fetch.get(&tx_id).copied().unwrap_or(0);
            let actual = to_cbor(tx).len() as u32;
            if actual != advertised {
                return protocol_error(TxSizeMismatch { tx_id, advertised, actual });
            }
        }

        Ok(None)
    }

    fn record_processed_results(&mut self, results: &[TxInsertResult]) {
        self.processed_fetch_set.extend(results.iter().map(TxInsertResult::tx_id).cloned());
    }

    /// Schedule a `CheckMempoolSize` self-message after a short delay if one isn't already
    /// pending. Called when we'd otherwise spin on req=0 round-trips because the mempool is full.
    async fn schedule_back_pressure_recheck(&mut self, eff: &Effects<Inputs<ResponderLocalIn>>) {
        if !self.back_pressure_scheduled {
            self.back_pressure_scheduled = true;
            eff.schedule_after(
                Inputs::Local(ResponderLocalIn::CheckMempoolSize),
                self.params.back_pressure_recheck_interval.as_duration(),
            )
            .await;
        }
    }

    /// Process an `InflightTimeout`. If the timeout `fetch_id` still matches `self.fetch_id`
    /// (i.e. no newer batch has been sent in the meantime) and we are still waiting for transactions
    /// we terminate the connection. Stale timeouts are simply ignored (we don't try to cancel them).
    fn handle_inflight_timeout(&self, fetch_id: u64, proto: &State) -> Option<ResponderAction> {
        if fetch_id == self.fetch_id && *proto == State::Txs {
            Some(ResponderAction::Error(TxFetchTimeout))
        } else {
            None
        }
    }

    /// Bump the fetch id and schedule a single `InflightTimeout` for the new batch.
    async fn schedule_inflight_timeout(&mut self, action: &ResponderAction, eff: &Effects<Inputs<ResponderLocalIn>>) {
        if matches!(action, ResponderAction::SendRequestTxs(_)) {
            self.fetch_id = self.fetch_id.wrapping_add(1);
            eff.schedule_after(
                Inputs::Local(ResponderLocalIn::InflightTimeout(self.fetch_id)),
                self.params.inflight_fetch_timeout.as_duration(),
            )
            .await;
        }
    }

    /// Re-attempt to drain `pending_fetch` after a back-pressure recheck fired.
    /// Resumes the protocol with a `RequestTxs` if mempool now has available capacity, otherwise schedules
    /// another recheck.
    async fn recheck_back_pressure(
        &mut self,
        mempool: &dyn TxSubmissionMempool<Transaction>,
        eff: &Effects<Inputs<ResponderLocalIn>>,
    ) -> Option<ResponderAction> {
        let txs = self.txs_to_request(mempool);
        if !txs.is_empty() {
            let action = ResponderAction::SendRequestTxs(txs);
            self.schedule_inflight_timeout(&action, eff).await;
            return Some(action);
        }

        // Still nothing fetchable: re-schedule the check while pending_fetch has entries.
        if !self.pending_fetch.is_empty() {
            self.schedule_back_pressure_recheck(eff).await;
            return None;
        }

        // pending_fetch drained naturally (e.g. txs landed in mempool via another peer) — re-engage
        // the peer with a normal RequestTxIds.
        let (ack, req, blocking) = self.request_tx_ids(mempool);
        Some(ResponderAction::SendRequestTxIds { ack, req, blocking })
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

fn protocol_error_outcome(error: ProtocolError) -> anyhow::Result<FetchOutcome> {
    tracing::warn!("protocol error: {error}");
    Ok(FetchOutcome::Action(ResponderAction::Error(error)))
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

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum TxSubmissionMsg {
    WaitForAtLeast {
        seq_no: MempoolSeqNo,
        caller: StageRef<()>,
    },
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

    use std::{collections::BTreeMap, sync::Arc};

    use amaru_kernel::Transaction;
    use amaru_mempool::strategies::InMemoryMempool;
    use amaru_ouroboros_traits::{
        mempool::overriding_mempool::OverridingMempool, MempoolError, MempoolSeqNo, TransactionValidationError,
        TxInsertResult, TxOrigin, TxRejectReason, TxSubmissionMempool,
    };

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
        let mempool = failing_insert_mempool("database unavailable");

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
        let mempool = mock_insert_mempool(vec![
            TxInsertResult::rejected(
                TxId::from(&txs[0]),
                TxRejectReason::Invalid(TransactionValidationError::from(anyhow::anyhow!("invalid for test"))),
            ),
            TxInsertResult::accepted(TxId::from(&txs[1]), MempoolSeqNo(1)),
        ]);

        let actions =
            run_stage(mempool, vec![init(), reply_tx_ids(&txs, &[0, 1, 2]), reply_txs(&txs, &[0, 1])]).await?;

        assert_actions_eq(
            &actions,
            &[request_tx_ids(0, 10, Blocking::Yes), request_txs(&txs, &[0, 1]), request_tx_ids(2, 9, Blocking::No)],
        );
        Ok(())
    }

    #[tokio::test]
    async fn body_with_mismatched_size_terminates_protocol() -> anyhow::Result<()> {
        let txs = create_transactions(2);
        let mempool = Arc::new(InMemoryMempool::default());

        // Advertise a deliberately wrong size for tx[0]
        let bad_advertisement = ResponderResult::ReplyTxIds(vec![(TxId::from(&txs[0]), 1), (TxId::from(&txs[1]), 1)]);
        let actions = run_stage(mempool, vec![init(), bad_advertisement, reply_txs(&txs, &[0])]).await?;

        let actual = to_cbor(&txs[0]).len() as u32;
        assert_actions_eq(
            &actions,
            &[
                request_tx_ids(0, 10, Blocking::Yes),
                request_txs(&txs, &[0, 1]),
                error_action(TxSizeMismatch { tx_id: TxId::from(&txs[0]), advertised: 1, actual }),
            ],
        );
        Ok(())
    }

    #[test]
    fn inflight_timeout_for_current_batch_in_txs_state_terminates_protocol() {
        let muxer = StageRef::<MuxMessage>::blackhole();
        let mempool_stage = StageRef::<MempoolMsg>::blackhole();
        let (_state, mut responder) = TxSubmissionResponder::new(muxer, test_params(), TxOrigin::Local, mempool_stage);

        responder.fetch_id = 5;
        let action = responder.handle_inflight_timeout(5, &State::Txs);
        assert_eq!(action, Some(ResponderAction::Error(TxFetchTimeout)));
    }

    #[test]
    fn inflight_timeout_with_stale_fetch_id_is_ignored() {
        // A timer scheduled for an earlier batch should not fire if a newer batch is now in flight.
        let muxer = StageRef::<MuxMessage>::blackhole();
        let mempool_stage = StageRef::<MempoolMsg>::blackhole();
        let (_state, mut responder) = TxSubmissionResponder::new(muxer, test_params(), TxOrigin::Local, mempool_stage);

        responder.fetch_id = 7;
        // Older timeout (fetch_id == 3) fires while we're already on fetch_id 7 — ignore.
        assert!(responder.handle_inflight_timeout(3, &State::Txs).is_none());
    }

    #[test]
    fn inflight_timeout_after_reply_received_is_ignored() {
        // ReplyTxs has been received, ignore the now stale timeout.
        let muxer = StageRef::<MuxMessage>::blackhole();
        let mempool_stage = StageRef::<MempoolMsg>::blackhole();
        let (_state, mut responder) = TxSubmissionResponder::new(muxer, test_params(), TxOrigin::Local, mempool_stage);

        responder.fetch_id = 5;
        assert!(responder.handle_inflight_timeout(5, &State::Idle).is_none());
    }

    #[test]
    fn process_tx_ids_reply_signals_back_pressure_when_mempool_full() {
        let txs = create_transactions(2);
        let muxer = StageRef::<MuxMessage>::blackhole();
        let mempool_stage = StageRef::<MempoolMsg>::blackhole();
        let (_state, mut responder) = TxSubmissionResponder::new(muxer, test_params(), TxOrigin::Local, mempool_stage);

        // Mempool reports zero capacity left, so any pending fetch should yield AwaitingCapacity.
        let mempool = full_mempool();

        let tx_ids = advertised(&txs, &[0, 1]);
        let outcome = responder.process_tx_ids_reply(mempool.as_ref(), tx_ids).expect("no protocol error");

        assert!(matches!(outcome, FetchOutcome::AwaitingCapacity), "expected AwaitingCapacity, got {outcome:?}");
        // pending_fetch retained for retry once capacity returns.
        assert_eq!(responder.pending_fetch.len(), 2, "pending_fetch should still hold both tx_ids");
    }

    #[test]
    fn process_tx_ids_reply_drains_after_recovery() {
        let txs = create_transactions(2);
        let muxer = StageRef::<MuxMessage>::blackhole();
        let mempool_stage = StageRef::<MempoolMsg>::blackhole();
        let (_state, mut responder) = TxSubmissionResponder::new(muxer, test_params(), TxOrigin::Local, mempool_stage);

        // First the mempool is full.
        let full = full_mempool();
        let tx_ids = advertised(&txs, &[0, 1]);
        let outcome = responder.process_tx_ids_reply(full.as_ref(), tx_ids).expect("no protocol error");
        assert!(matches!(outcome, FetchOutcome::AwaitingCapacity));

        // Then the mempool drains and we can fetch transactions
        let mempool: Arc<dyn TxSubmissionMempool<Transaction>> = Arc::new(InMemoryMempool::default());
        let txs_to_fetch = responder.txs_to_request(mempool.as_ref());
        assert_eq!(txs_to_fetch.len(), 2, "both pending tx_ids should now be fetched");
    }

    /// Build a `(TxId, advertised_size)` list for the given transactions, with sizes computed
    /// from the actual CBOR encoding (so the byte-budget logic in `txs_to_request` is realistic).
    fn advertised(txs: &[Transaction], indexes: &[usize]) -> Vec<(TxId, u32)> {
        indexes.iter().map(|i| (TxId::from(&txs[*i]), to_cbor(&txs[*i]).len() as u32)).collect()
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

    async fn run_stage<M: AsyncMempool>(
        mempool: Arc<M>,
        results: Vec<ResponderResult>,
    ) -> anyhow::Result<Vec<ResponderAction>> {
        Ok(run_stage_and_return_state(mempool, results).await?.0)
    }

    /// Build `ResponderParams` sized for the protocol-trace tests: window of 10, byte budget that
    /// fits exactly two synthetic test transactions per `RequestTxs` round (the previous count
    /// equivalent of `fetch_batch=2`). Computed at call time so it stays correct if the synthetic
    /// tx encoding changes. Production defaults (`393_240` bytes) are exercised via `Config`.
    fn test_params() -> ResponderParams {
        let sample_size = to_cbor(&crate::tx_submission::tests::create_transaction(0)).len() as u64;
        let max_window = std::num::NonZeroU16::new(10).expect("test max_window must be non-zero");
        let fetch_batch_bytes =
            std::num::NonZeroU64::new(2 * sample_size).expect("test fetch_batch_bytes must be non-zero");
        ResponderParams::new(max_window, fetch_batch_bytes)
    }

    /// Run the responder stage, given a list of ResponderResults as inputs, and return the list of
    /// ResponderActions produced as output, plus the responder itself.
    async fn run_stage_and_return_state(
        mempool: Arc<dyn TxSubmissionMempool<Transaction>>,
        results: Vec<ResponderResult>,
    ) -> anyhow::Result<(Vec<ResponderAction>, TxSubmissionResponder)> {
        run_stage_and_return_state_with(
            TxSubmissionResponder::new(
                StageRef::named_for_tests("muxer"),
                test_params(),
                TxOrigin::Local,
                StageRef::blackhole(),
            )
            .1,
            mempool,
            results,
        )
        .await
    }

    async fn run_stage_and_return_state_with<M: AsyncMempool>(
        mut responder: TxSubmissionResponder,
        mempool: Arc<M>,
        results: Vec<ResponderResult>,
    ) -> anyhow::Result<(Vec<ResponderAction>, TxSubmissionResponder)> {
        let mut actions = vec![];
        let mempool = mempool.as_ref();
        for r in results {
            let action = match r {
                ResponderResult::Init => responder.initialize_state(mempool).await,
                ResponderResult::ReplyTxIds(tx_ids) => {
                    match responder.process_tx_ids_reply(mempool, tx_ids).await? {
                        FetchOutcome::Action(a) => Some(a),
                        // Tests use unbounded mempools, so this branch is unreachable here.
                        FetchOutcome::AwaitingCapacity => None,
                    }
                }
                ResponderResult::ReplyTxs(txs) => {
                    if let Some(action) = responder.validate_received_txs(&txs)? {
                        Some(action)
                    } else {
                        let origin = responder.origin.clone();
                        let mut processed_results = Vec::with_capacity(txs.len());
                        let mut error = None;
                        for tx in txs {
                            let requested_id = TxId::from(&tx);
                            responder.inflight_fetch.remove(&requested_id);
                            match mempool.insert(tx, origin.clone()).await {
                                Ok(result) => processed_results.push(result),
                                Err(e) => {
                                    error = Some(protocol_error(MempoolInsertFailed(requested_id, e))?);
                                    break;
                                }
                            }
                        }
                        if let Some(error) = error {
                            error
                        } else {
                            responder.record_processed_results(&processed_results);
                            let (ack, req, blocking) = responder.request_tx_ids(mempool).await;
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
        ResponderResult::ReplyTxIds(
            ids.iter().map(|id| (TxId::from(&txs[*id]), to_cbor(&txs[*id]).len() as u32)).collect(),
        )
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

    /// A mempool that fails every `insert` with the given error message
    fn failing_insert_mempool(message: &'static str) -> Arc<dyn TxSubmissionMempool<Transaction>> {
        Arc::new(
            OverridingMempool::builder(Arc::new(InMemoryMempool::default()))
                .with_insert(move |_inner, _tx, _origin| Err(MempoolError::new(message)))
                .build(),
        )
    }

    /// A mempool whose `insert` returns predetermined results keyed by `TxId`.
    /// Each result is consumed on first match.
    fn mock_insert_mempool(results: Vec<TxInsertResult>) -> Arc<dyn TxSubmissionMempool<Transaction>> {
        let mut by_id: BTreeMap<TxId, TxInsertResult> =
            results.into_iter().map(|result| (*result.tx_id(), result)).collect();
        Arc::new(
            OverridingMempool::builder(Arc::new(InMemoryMempool::default()))
                .with_insert(move |_inner, tx, _origin| {
                    let tx_id = TxId::from(&tx);
                    Ok(by_id.remove(&tx_id).expect("missing insert result for mock mempool test"))
                })
                // never report contained txs to force the responder to fetch them.
                .with_contains(|_inner, _tx_id| false)
                .build(),
        )
    }

    /// A mempool that always reports being at capacity, used to drive back-pressure tests.
    fn full_mempool() -> Arc<dyn TxSubmissionMempool<Transaction>> {
        Arc::new(
            OverridingMempool::builder(Arc::new(InMemoryMempool::default()))
                .with_is_near_capacity(|_inner, _additional| true)
                .build(),
        )
    }
}
