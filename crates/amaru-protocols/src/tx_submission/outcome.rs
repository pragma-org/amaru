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

use crate::tx_submission::{ResponderResult, initiator::InitiatorResult};
use amaru_ouroboros_traits::TxId;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};

/// Outcome of a protocol state machine step
#[derive(Debug)]
pub enum Outcome {
    Done,
    Error(ProtocolError),
    Initiator(InitiatorResult),
    Responder(ResponderResult),
}

#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize)]
pub enum ProtocolError {
    NoTxIdsRequested,
    BlockingRequestMadeWhenTxsStillUnacknowledged,
    NonBlockingRequestMadeWhenAllTxsAcknowledged,
    NoAckOrReqTxIdsRequested,
    NoTxsRequested,
    UnadvertisedTransactionIdsRequested(Vec<TxId>),
    UnknownTxsRequested(Vec<TxId>),
    DuplicateTxIds(Vec<TxId>),
    MaxOutstandingTxIdsRequested(u16, u16),
    TooManyAcknowledgedTxs(u16, u16),
    TooManyTxIdsReceived(usize, usize, usize),
    ReceivedTxsExceedsBatchSize(usize, usize),
    SomeReceivedTxsNotInFlight(Vec<TxId>),
}

impl Display for ProtocolError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtocolError::NoTxIdsRequested => write!(f, "0 tx ids were requested"),
            ProtocolError::BlockingRequestMadeWhenTxsStillUnacknowledged => {
                write!(
                    f,
                    "a non-blocking request must be made when some txs are unacknowledged"
                )
            }
            ProtocolError::NonBlockingRequestMadeWhenAllTxsAcknowledged => {
                write!(
                    f,
                    "a blocking request must be made when all the txs are acknowledged"
                )
            }
            ProtocolError::NoAckOrReqTxIdsRequested => {
                write!(
                    f,
                    "0 transactions acknowledged and 0 transaction ids requested"
                )
            }
            ProtocolError::NoTxsRequested => write!(f, "0 transactions were requested"),
            ProtocolError::UnadvertisedTransactionIdsRequested(tx_ids) => {
                write!(f, "unadvertised transaction ids requested: {:?}", tx_ids)
            }
            ProtocolError::UnknownTxsRequested(tx_ids) => {
                write!(f, "unknown transactions requested: {:?}", tx_ids)
            }
            ProtocolError::MaxOutstandingTxIdsRequested(req, limit) => {
                write!(
                    f,
                    "the number of requested transaction ids exceeds the protocol limit: {req} > {limit}"
                )
            }
            ProtocolError::TooManyAcknowledgedTxs(ack, window_len) => {
                write!(
                    f,
                    "it is not possible to acknowledge more transactions than currently unacknowledged: {ack} > {window_len}"
                )
            }
            ProtocolError::TooManyTxIdsReceived(received, current_window_len, max_window_len) => {
                write!(
                    f,
                    "the number of received transaction ids exceeds the max window size: {received} + {current_window_len} > {max_window_len}"
                )
            }
            ProtocolError::ReceivedTxsExceedsBatchSize(received, max) => {
                write!(
                    f,
                    "the number of received transactions exceeds the configured batch size: {received} > {max}"
                )
            }
            ProtocolError::SomeReceivedTxsNotInFlight(tx_ids) => {
                write!(
                    f,
                    "some received transactions were not requested: {tx_ids:?}"
                )
            }
            ProtocolError::DuplicateTxIds(tx_ids) => {
                write!(
                    f,
                    "duplicate transaction ids were found in the request: {tx_ids:?}"
                )
            }
        }
    }
}

impl PartialEq for Outcome {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Outcome::Done, Outcome::Done) => true,
            (Outcome::Error(e1), Outcome::Error(e2)) => e1 == e2,
            (Outcome::Initiator(m1), Outcome::Initiator(m2)) => m1 == m2,
            (Outcome::Responder(m1), Outcome::Responder(m2)) => m1 == m2,
            _ => false,
        }
    }
}

impl Eq for Outcome {}

impl Display for Outcome {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Outcome::Done => write!(f, "Done"),
            Outcome::Error(err) => write!(f, "ProtocolError({})", err),
            Outcome::Initiator(msg) => write!(f, "Initiator({})", msg),
            Outcome::Responder(msg) => write!(f, "Responder({})", msg),
        }
    }
}
