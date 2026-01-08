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

/// Manages the transaction submission protocol from the initiator's perspective.
///
/// This module implements the initiator side of the Cardano transaction submission protocol (N2N),
/// responsible for requesting transaction IDs from a responder and then requesting the full
/// transactions based on those IDs.
///
/// # Protocol Flow
///
/// The initiator follows this state machine:
/// - **Init**: Sends an initialization message
/// - **Idle**: Waits for requests from the protocol layer
/// - **TxIdsBlocking**: Waits for transaction IDs in a blocking manner (waits for mempool to have data)
/// - **TxIdsNonBlocking**: Waits for transaction IDs in a non-blocking manner (returns immediately)
/// - **Txs**: Waits for full transaction responses
///
/// # Key Components
///
/// - [`TxSubmissionInitiator`]: The main state machine that tracks advertised transactions
/// - [`InitiatorAction`]: Actions produced by the initiator (send replies or errors)
/// - [`InitiatorResult`]: Results from processing network messages
///
/// # Window Management
///
/// The initiator maintains a sliding window of advertised transaction IDs. Transactions are:
/// - Added to the window when they're sent to the responder
/// - Removed from the window when they're acknowledged by the responder
/// - Tracked with their mempool sequence numbers to prevent re-advertisement
///
/// # Error Handling
///
/// The protocol validates:
/// - Request counts don't exceed maximum protocol limits
/// - Acknowledgment counts don't exceed advertised transactions
/// - Only advertised transaction IDs are requested
/// - Appropriate blocking/non-blocking requests based on acknowledgment state
use std::collections::VecDeque;
use std::fmt::{Debug, Display};

use crate::mempool_effects::MemoryPool;
use crate::mux::MuxMessage;
use crate::protocol::{
    Initiator, Inputs, Miniprotocol, Outcome, PROTO_N2N_TX_SUB, ProtocolState, StageState,
    miniprotocol, outcome,
};
use crate::tx_submission::{Blocking, Message, ProtocolError, TxSubmissionState};
use ProtocolError::*;
use amaru_kernel::Tx;
use amaru_ouroboros::{MempoolSeqNo, TxSubmissionMempool};
use amaru_ouroboros_traits::TxId;
use pure_stage::{DeserializerGuards, Effects, StageRef, Void};

const MAX_REQUESTED_TX_IDS: u16 = 10;

pub fn register_deserializers() -> DeserializerGuards {
    vec![
        pure_stage::register_data_deserializer::<Void>().boxed(),
        pure_stage::register_data_deserializer::<TxSubmissionInitiator>().boxed(),
    ]
}

pub fn initiator() -> Miniprotocol<TxSubmissionState, TxSubmissionInitiator, Initiator> {
    miniprotocol(PROTO_N2N_TX_SUB)
}

impl StageState<TxSubmissionState, Initiator> for TxSubmissionInitiator {
    type LocalIn = Void;

    async fn local(
        self,
        _proto: &TxSubmissionState,
        _input: Self::LocalIn,
        _eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<InitiatorAction>, Self)> {
        // Currently no local inputs are handled
        Ok((None, self))
    }

    async fn network(
        mut self,
        _proto: &TxSubmissionState,
        input: InitiatorResult,
        eff: &Effects<Inputs<Self::LocalIn>>,
    ) -> anyhow::Result<(Option<InitiatorAction>, Self)> {
        let mempool: &dyn TxSubmissionMempool<Tx> = &MemoryPool::new(eff.clone());

        let action = match input {
            InitiatorResult::RequestTxIds {
                ack,
                req,
                blocking: Blocking::Yes,
            } => self.request_tx_ids_blocking(mempool, ack, req).await?,
            InitiatorResult::RequestTxIds {
                ack,
                req,
                blocking: Blocking::No,
            } => self.request_tx_ids_non_blocking(mempool, ack, req)?,
            InitiatorResult::RequestTxs(tx_ids) => self.request_txs(mempool, tx_ids)?,
        };
        Ok((action, self))
    }

    fn muxer(&self) -> &StageRef<MuxMessage> {
        &self.muxer
    }
}

impl ProtocolState<Initiator> for TxSubmissionState {
    type WireMsg = Message;
    type Action = InitiatorAction;
    type Out = InitiatorResult;

    fn init(&self) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out>, Self)> {
        Ok((outcome().send(Message::Init), TxSubmissionState::Idle))
    }

    fn network(
        &self,
        input: Self::WireMsg,
    ) -> anyhow::Result<(Outcome<Self::WireMsg, Self::Out>, Self)> {
        Ok(match (self, input) {
            (TxSubmissionState::Idle, Message::RequestTxIdsBlocking(ack, req)) => {
                tracing::trace!(ack=%ack, req=%req, "received RequestTxIdsBlocking");
                (
                    outcome().want_next().result(InitiatorResult::RequestTxIds {
                        ack,
                        req,
                        blocking: Blocking::Yes,
                    }),
                    TxSubmissionState::TxIdsBlocking,
                )
            }
            (TxSubmissionState::Idle, Message::RequestTxIdsNonBlocking(ack, req)) => {
                tracing::trace!(ack=%ack, req=%req, "received RequestTxIdsNonBlocking");
                (
                    outcome().want_next().result(InitiatorResult::RequestTxIds {
                        ack,
                        req,
                        blocking: Blocking::No,
                    }),
                    TxSubmissionState::TxIdsNonBlocking,
                )
            }
            (TxSubmissionState::Idle, Message::RequestTxs(tx_ids)) => {
                tracing::trace!(tx_ids_nb = tx_ids.len(), "received RequestTxs");
                (
                    outcome()
                        .want_next()
                        .result(InitiatorResult::RequestTxs(tx_ids)),
                    TxSubmissionState::Txs,
                )
            }
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }

    fn local(&self, action: Self::Action) -> anyhow::Result<(Outcome<Self::WireMsg, Void>, Self)> {
        Ok(match (self, action) {
            (TxSubmissionState::TxIdsBlocking, InitiatorAction::SendReplyTxIds(tx_ids)) => (
                outcome().send(Message::ReplyTxIds(tx_ids)),
                TxSubmissionState::Idle,
            ),
            (TxSubmissionState::TxIdsNonBlocking, InitiatorAction::SendReplyTxIds(tx_ids)) => (
                outcome().send(Message::ReplyTxIds(tx_ids)),
                TxSubmissionState::Idle,
            ),
            (TxSubmissionState::Txs, InitiatorAction::SendReplyTxs(txs)) => (
                outcome().send(Message::ReplyTxs(txs)),
                TxSubmissionState::Idle,
            ),
            (TxSubmissionState::Idle, InitiatorAction::Done) => {
                (outcome().send(Message::Done), TxSubmissionState::Done)
            }
            (this, input) => anyhow::bail!("invalid state: {:?} <- {:?}", this, input),
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum InitiatorAction {
    SendReplyTxIds(Vec<(TxId, u32)>),
    SendReplyTxs(Vec<Tx>),
    Error(ProtocolError),
    Done,
}

impl Display for InitiatorAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InitiatorAction::SendReplyTxIds(tx_ids) => {
                write!(f, "SendReplyTxIds(len={})", tx_ids.len())
            }
            InitiatorAction::SendReplyTxs(txs) => write!(f, "SendReplyTxs(len={})", txs.len()),
            InitiatorAction::Error(err) => write!(f, "Error({})", err),
            InitiatorAction::Done => write!(f, "Done"),
        }
    }
}

/// Result from protocol state when network message is received
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum InitiatorResult {
    RequestTxIds {
        ack: u16,
        req: u16,
        blocking: Blocking,
    },
    RequestTxs(Vec<TxId>),
}

impl Display for InitiatorResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InitiatorResult::RequestTxIds { ack, req, blocking } => {
                write!(
                    f,
                    "RequestTxIds(ack: {}, req: {}, blocking: {:?})",
                    ack, req, blocking
                )
            }
            InitiatorResult::RequestTxs(tx_ids) => {
                write!(
                    f,
                    "RequestTxs(ids: [{}])",
                    tx_ids
                        .iter()
                        .map(|id| format!("{}", id))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TxSubmissionInitiator {
    /// What we’ve already advertised but has not yet been fully acked.
    window: VecDeque<(TxId, MempoolSeqNo)>,
    /// Last seq_no we have ever pulled from the mempool for this peer.
    /// None if we have not pulled anything yet.
    last_seq: Option<MempoolSeqNo>,
    muxer: StageRef<MuxMessage>,
}

impl TxSubmissionInitiator {
    pub fn new(muxer: StageRef<MuxMessage>) -> (TxSubmissionState, Self) {
        (
            TxSubmissionState::Init,
            Self {
                window: VecDeque::new(),
                last_seq: None,
                muxer,
            },
        )
    }

    async fn request_tx_ids_blocking(
        &mut self,
        mempool: &dyn TxSubmissionMempool<Tx>,
        ack: u16,
        req: u16,
    ) -> anyhow::Result<Option<InitiatorAction>> {
        // check the ack and req values
        tracing::trace!(?ack, ?req, "received RequestTxIdsBlocking");
        if req == 0 {
            return protocol_error(NoTxIdsRequested);
        };
        if let Some(value) = self.check_ack_req(ack, req) {
            return protocol_error(value);
        }
        if (ack as usize) < self.window.len() {
            return protocol_error(BlockingRequestMadeWhenTxsStillUnacknowledged);
        }

        // update the window by discarding acknowledged tx ids and update the last_seq
        self.discard(ack);
        if !mempool
            .wait_for_at_least(self.last_seq.unwrap_or_default().add(req as u64))
            .await
        {
            return Ok(None);
        }
        let tx_ids = self.get_next_tx_ids(mempool, req)?;
        Ok(Some(InitiatorAction::SendReplyTxIds(tx_ids)))
    }

    fn request_tx_ids_non_blocking(
        &mut self,
        mempool: &dyn TxSubmissionMempool<Tx>,
        ack: u16,
        req: u16,
    ) -> anyhow::Result<Option<InitiatorAction>> {
        // check the ack and req values
        tracing::trace!(?ack, ?req, "received RequestTxIdsNonBlocking");
        if ack == 0 && req == 0 {
            return protocol_error(NoAckOrReqTxIdsRequested);
        }
        if let Some(error) = self.check_ack_req(ack, req) {
            return protocol_error(error);
        }
        if ack as usize == self.window.len() {
            return protocol_error(NonBlockingRequestMadeWhenAllTxsAcknowledged);
        }

        // update the window by discarding acknowledged tx ids and update the last_seq
        self.discard(ack);
        Ok(Some(InitiatorAction::SendReplyTxIds(
            self.get_next_tx_ids(mempool, req)?,
        )))
    }

    fn request_txs(
        &mut self,
        mempool: &dyn TxSubmissionMempool<Tx>,
        tx_ids: Vec<TxId>,
    ) -> anyhow::Result<Option<InitiatorAction>> {
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
            Ok(Some(InitiatorAction::SendReplyTxs(txs)))
        }
    }

    /// Check that the ack and req values are valid for a request whether it is blocking or non blocking.
    fn check_ack_req(&mut self, ack: u16, req: u16) -> Option<ProtocolError> {
        if req > MAX_REQUESTED_TX_IDS {
            Some(MaxOutstandingTxIdsRequested(req, MAX_REQUESTED_TX_IDS))
        } else if ack as usize > self.window.len() {
            Some(TooManyAcknowledgedTxs(ack, self.window.len() as u16))
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

fn protocol_error(error: ProtocolError) -> anyhow::Result<Option<InitiatorAction>> {
    tracing::warn!("protocol error: {error}");
    Ok(Some(InitiatorAction::Error(error)))
}

impl AsRef<StageRef<MuxMessage>> for TxSubmissionInitiator {
    fn as_ref(&self) -> &StageRef<MuxMessage> {
        &self.muxer
    }
}

#[cfg(test)]
mod tests {
    // TODO: Implement protocol spec check similar to keepalive
    // This will require creating a spec() function first

    use super::*;
    use crate::{
        protocol::Role,
        tx_submission::{
            assert_actions_eq, create_transactions_in_mempool,
            tests::{SizedMempool, create_transactions},
        },
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
        let results = vec![
            request_tx_ids(0, 2, Blocking::Yes),
            request_txs(&txs, &[0, 1]),
            request_tx_ids(1, 2, Blocking::No), // line 1
            request_txs(&txs, &[2, 3]),
            request_tx_ids(3, 2, Blocking::Yes), // line 2
            request_txs(&txs, &[4, 5]),
            request_tx_ids(2, 2, Blocking::Yes),
        ];

        let outcomes = run_stage(mempool, results).await?;

        // Check replies
        // We basically assert that we receive the expected ids and transactions
        // 2 by 2, then the last one, since we requested batches of 2.
        assert_actions_eq(
            &outcomes,
            &[
                reply_tx_ids(&txs, &[0, 1]),
                reply_txs(&txs, &[0, 1]),
                reply_tx_ids(&txs, &[2, 3]),
                reply_txs(&txs, &[2, 3]),
                reply_tx_ids(&txs, &[4, 5]),
                reply_txs(&txs, &[4, 5]),
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
        let results = vec![
            request_tx_ids(0, 2, Blocking::Yes),
            request_txs(&txs, &[0, 1]),
            request_tx_ids(1, 2, Blocking::No),
        ];

        let (actions, initiator) = run_stage_and_return_state(mempool.clone(), results).await?;
        assert_actions_eq(
            &actions,
            &[
                reply_tx_ids(&txs, &[0, 1]),
                reply_txs(&txs, &[0, 1]),
                reply_tx_ids(&txs, &[]),
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

        let (actions, _) = run_stage_and_return_state_with(initiator, mempool, messages).await?;

        // Check replies
        // We basically assert that we receive the expected ids and transactions
        // 2 by 2, then the last one, since we requested batches of 2.
        assert_actions_eq(
            &actions,
            &[
                reply_tx_ids(&txs, &[2, 3]),
                reply_txs(&txs, &[2, 3]),
                reply_tx_ids(&txs, &[4, 5]),
                reply_txs(&txs, &[4, 5]),
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
        let results = vec![
            request_tx_ids(0, 2, Blocking::Yes),
            request_txs(&txs, &[2, 3]),
        ];

        let actions = run_stage(mempool, results).await?;
        assert_actions_eq(
            &actions,
            &[
                reply_tx_ids(&txs, &[0, 1]),
                error_action(UnadvertisedTransactionIdsRequested(vec![
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

        let results = vec![request_tx_ids(0, 0, Blocking::Yes)];
        let actions = run_stage(mempool, results).await?;
        assert_actions_eq(&actions, &[error_action(NoTxIdsRequested)]);
        Ok(())
    }

    #[tokio::test]
    async fn blocking_requested_txs_must_be_greater_than_0() -> anyhow::Result<()> {
        let mempool = Arc::new(SizedMempool::with_capacity(4));
        let txs = create_transactions_in_mempool(mempool.clone(), 4);

        let results = vec![request_tx_ids(0, 2, Blocking::Yes), request_txs(&txs, &[])];

        let actions = run_stage(mempool, results).await?;
        assert_actions_eq(
            &actions,
            &[reply_tx_ids(&txs, &[0, 1]), error_action(NoTxsRequested)],
        );
        Ok(())
    }

    #[tokio::test]
    async fn non_blocking_ack_or_requested_ids_must_be_greater_than_0() -> anyhow::Result<()> {
        let mempool = Arc::new(SizedMempool::with_capacity(6));

        let results = vec![request_tx_ids(0, 0, Blocking::No)];
        let actions = run_stage(mempool, results).await?;
        assert_actions_eq(&actions, &[error_action(NoAckOrReqTxIdsRequested)]);
        Ok(())
    }

    #[tokio::test]
    async fn blocking_requested_nb_must_be_less_than_protocol_limit() -> anyhow::Result<()> {
        let mempool = Arc::new(SizedMempool::with_capacity(6));

        let results = vec![request_tx_ids(0, 12, Blocking::Yes)];
        let actions = run_stage(mempool, results).await?;
        assert_actions_eq(
            &actions,
            &[error_action(MaxOutstandingTxIdsRequested(
                12,
                MAX_REQUESTED_TX_IDS,
            ))],
        );
        Ok(())
    }

    #[tokio::test]
    async fn non_blocking_requested_nb_must_be_less_than_protocol_limit() -> anyhow::Result<()> {
        let mempool = Arc::new(SizedMempool::with_capacity(6));

        let results = vec![request_tx_ids(0, 12, Blocking::No)];
        let actions = run_stage(mempool, results).await?;
        assert_actions_eq(
            &actions,
            &[error_action(MaxOutstandingTxIdsRequested(
                12,
                MAX_REQUESTED_TX_IDS,
            ))],
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_blocking_request_must_be_made_when_all_txs_are_acknowledged() -> anyhow::Result<()> {
        let mempool = Arc::new(SizedMempool::with_capacity(4));
        let txs = create_transactions_in_mempool(mempool.clone(), 4);

        let results = vec![
            request_tx_ids(0, 4, Blocking::Yes),
            request_txs(&txs, &[0, 1]),
            request_tx_ids(2, 4, Blocking::No),
            request_txs(&txs, &[2, 3]),
            request_tx_ids(2, 4, Blocking::No),
        ];
        let actions = run_stage(mempool, results).await?;
        assert_actions_eq(
            &actions,
            &[
                reply_tx_ids(&txs, &[0, 1, 2, 3]),
                reply_txs(&txs, &[0, 1]),
                reply_tx_ids(&txs, &[]),
                reply_txs(&txs, &[2, 3]),
                error_action(NonBlockingRequestMadeWhenAllTxsAcknowledged),
            ],
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_non_blocking_request_must_be_made_when_some_txs_are_unacknowledged()
    -> anyhow::Result<()> {
        let mempool = Arc::new(SizedMempool::with_capacity(4));
        let txs = create_transactions_in_mempool(mempool.clone(), 4);

        let results = vec![
            request_tx_ids(0, 4, Blocking::Yes),
            request_txs(&txs, &[0, 1]),
            request_tx_ids(2, 4, Blocking::Yes),
        ];
        let actions = run_stage(mempool, results).await?;
        assert_actions_eq(
            &actions,
            &[
                reply_tx_ids(&txs, &[0, 1, 2, 3]),
                reply_txs(&txs, &[0, 1]),
                error_action(BlockingRequestMadeWhenTxsStillUnacknowledged),
            ],
        );
        Ok(())
    }

    #[tokio::test]
    async fn the_responder_cannot_acknowledge_more_than_the_current_unacknowledged_blocking()
    -> anyhow::Result<()> {
        let mempool = Arc::new(SizedMempool::with_capacity(4));
        let txs = create_transactions_in_mempool(mempool.clone(), 4);

        let results = vec![
            request_tx_ids(0, 4, Blocking::Yes),
            request_txs(&txs, &[0, 1]),
            request_tx_ids(2, 4, Blocking::No),
            request_txs(&txs, &[2, 3]),
            request_tx_ids(4, 4, Blocking::Yes),
        ];
        let actions = run_stage(mempool, results).await?;
        assert_actions_eq(
            &actions,
            &[
                reply_tx_ids(&txs, &[0, 1, 2, 3]),
                reply_txs(&txs, &[0, 1]),
                reply_tx_ids(&txs, &[]),
                reply_txs(&txs, &[2, 3]),
                error_action(TooManyAcknowledgedTxs(4, 2)),
            ],
        );
        Ok(())
    }

    #[tokio::test]
    async fn the_responder_cannot_acknowledge_more_than_the_current_unacknowledged_non_blocking()
    -> anyhow::Result<()> {
        let mempool = Arc::new(SizedMempool::with_capacity(4));
        let txs = create_transactions_in_mempool(mempool.clone(), 4);

        let results = vec![
            request_tx_ids(0, 4, Blocking::Yes),
            request_txs(&txs, &[0, 1]),
            request_tx_ids(2, 4, Blocking::No),
            request_txs(&txs, &[2, 3]),
            request_tx_ids(4, 4, Blocking::No),
        ];
        let actions = run_stage(mempool, results).await?;
        assert_actions_eq(
            &actions,
            &[
                reply_tx_ids(&txs, &[0, 1, 2, 3]),
                reply_txs(&txs, &[0, 1]),
                reply_tx_ids(&txs, &[]),
                reply_txs(&txs, &[2, 3]),
                error_action(TooManyAcknowledgedTxs(4, 2)),
            ],
        );
        Ok(())
    }

    #[test]
    fn test_initiator_protocol() {
        crate::tx_submission::spec::<Initiator>().check(
            TxSubmissionState::Init,
            Role::Initiator,
            |msg| match msg {
                Message::ReplyTxIds(tx_ids) => {
                    Some(InitiatorAction::SendReplyTxIds(tx_ids.clone()))
                }
                Message::ReplyTxs(txs) => Some(InitiatorAction::SendReplyTxs(txs.clone())),
                Message::Done => Some(InitiatorAction::Done),
                Message::Init
                | Message::RequestTxs(_)
                | Message::RequestTxIdsBlocking(_, _)
                | Message::RequestTxIdsNonBlocking(_, _) => None,
            },
            |msg| msg.clone(),
        );
    }

    // HELPERS

    async fn run_stage(
        mempool: Arc<dyn TxSubmissionMempool<Tx>>,
        results: Vec<InitiatorResult>,
    ) -> anyhow::Result<Vec<InitiatorAction>> {
        let (actions, _initiator) = run_stage_and_return_state(mempool, results).await?;
        Ok(actions)
    }

    async fn run_stage_and_return_state(
        mempool: Arc<dyn TxSubmissionMempool<Tx>>,
        results: Vec<InitiatorResult>,
    ) -> anyhow::Result<(Vec<InitiatorAction>, TxSubmissionInitiator)> {
        run_stage_and_return_state_with(
            TxSubmissionInitiator::new(StageRef::named_for_tests("muxer")).1,
            mempool,
            results,
        )
        .await
    }

    async fn run_stage_and_return_state_with(
        mut initiator: TxSubmissionInitiator,
        mempool: Arc<dyn TxSubmissionMempool<Tx>>,
        results: Vec<InitiatorResult>,
    ) -> anyhow::Result<(Vec<InitiatorAction>, TxSubmissionInitiator)> {
        let mut actions = vec![];
        for r in results {
            let action = step(&mut initiator, r, mempool.as_ref()).await?;
            if let Some(action) = action {
                actions.push(action);
            }
        }
        Ok((actions, initiator))
    }

    async fn step(
        initiator: &mut TxSubmissionInitiator,
        input: InitiatorResult,
        mempool: &dyn TxSubmissionMempool<Tx>,
    ) -> anyhow::Result<Option<InitiatorAction>> {
        let action = match input {
            InitiatorResult::RequestTxIds {
                ack,
                req,
                blocking: Blocking::Yes,
            } => initiator.request_tx_ids_blocking(mempool, ack, req).await?,
            InitiatorResult::RequestTxIds {
                ack,
                req,
                blocking: Blocking::No,
            } => initiator.request_tx_ids_non_blocking(mempool, ack, req)?,
            InitiatorResult::RequestTxs(tx_ids) => initiator.request_txs(mempool, tx_ids)?,
        };
        Ok(action)
    }

    fn reply_tx_ids(txs: &[Tx], ids: &[usize]) -> InitiatorAction {
        InitiatorAction::SendReplyTxIds(ids.iter().map(|id| (TxId::from(&txs[*id]), 50)).collect())
    }

    fn reply_txs(txs: &[Tx], ids: &[usize]) -> InitiatorAction {
        InitiatorAction::SendReplyTxs(ids.iter().map(|id| txs[*id].clone()).collect())
    }

    fn request_tx_ids(ack: u16, req: u16, blocking: Blocking) -> InitiatorResult {
        InitiatorResult::RequestTxIds { ack, req, blocking }
    }

    fn request_txs(txs: &[Tx], ids: &[usize]) -> InitiatorResult {
        InitiatorResult::RequestTxs(ids.iter().map(|id| TxId::from(&txs[*id])).collect())
    }

    fn error_action(error: ProtocolError) -> InitiatorAction {
        InitiatorAction::Error(error)
    }
}
