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

use std::fmt::{Display, Formatter};

use amaru_ouroboros_traits::TxId;
use serde::{Deserialize, Serialize};

use crate::tx_submission::{ResponderResult, initiator::InitiatorResult};

/// Outcome of a protocol state machine step
#[derive(Debug)]
pub enum Outcome {
    Done,
    Error(TerminationCause),
    Initiator(InitiatorResult),
    Responder(ResponderResult),
}

/// Reasons the tx-submission protocol may terminate the connection.
///
/// Splits peer-misbehavior errors (`Protocol`) from other operational issues.
#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize)]
pub enum TerminationCause {
    /// The peer violated the wire protocol.
    Protocol(ProtocolError),
    /// The peer did not deliver `ReplyTxs` within the inflight timeout.
    /// This should be modelled at the protocol level.
    TxFetchTimeout,
    /// Local mempool stage failed or did not respond.
    MempoolStageUnavailable,
}

/// Peer-misbehavior protocol errors.
///
/// - Responder side: check incoming `ReplyTxIds` / `ReplyTxs` messages.
/// - Initiator side: check incoming `RequestTxIds` / `RequestTxs` messages.
///
#[derive(Debug, PartialEq, Eq, Clone, Serialize, Deserialize)]
pub enum ProtocolError {
    // RESPONDER CHECKS
    /// One or more transaction bodies were not requested in the corresponding `RequestTxs` message.
    TxNotRequested,
    /// More tx ids were returned than we asked for.
    TxIdsNotRequested,
    /// A reply to a blocking request must always return tx_ids.
    TxIdsEmptyInBlockingReply,
    /// One or more received tx bodies had a CBOR size that did not match the size advertised
    /// in `ReplyTxIds` (beyond `MAX_TX_SIZE_DISCREPANCY`).
    TxSizeError(Vec<TxSizeMismatch>),

    // INITIATOR CHECKS
    /// More tx_ids were acknowledged than are currently outstanding.
    AckedTooManyTxids,
    /// Zero work was asked: `req == 0` on a blocking `RequestTxIds`, or
    /// `ack == 0 && req == 0` on a non-blocking one.
    RequestedNothing,
    /// The number of requested transactions would push the total in flight past the window cap.
    RequestedTooManyTxIds { req: u16, unacked: u16, max_unacked: u16 },
    /// A blocking `RequestTxIds` request was made while there were still unacknowledged ids
    /// (it should have used a non-blocking one).
    RequestBlocking,
    /// A non-blocking `RequestTxIds` request was made when there were no unacknowledged ids
    /// (it should have used a blocking one).
    RequestNonBlocking,
    /// Peer asked for a transaction that has not been advertised or already been acknowledged.
    /// Note that it is ok to re-request a transaction that is still unacked.
    RequestedUnavailableTx,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, Serialize, Deserialize)]
pub struct TxSizeMismatch {
    pub tx_id: TxId,
    pub advertised: u32,
    pub actual: u32,
}

impl Display for TerminationCause {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            TerminationCause::Protocol(err) => write!(f, "{err}"),
            TerminationCause::TxFetchTimeout => write!(f, "peer did not deliver ReplyTxs within timeout"),
            TerminationCause::MempoolStageUnavailable => write!(f, "mempool stage unavailable"),
        }
    }
}

impl Display for ProtocolError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtocolError::TxNotRequested => write!(f, "peer sent transactions we did not request"),
            ProtocolError::TxIdsNotRequested => write!(f, "peer replied with more tx ids than we asked for"),
            ProtocolError::TxSizeError(mismatches) => {
                write!(f, "peer sent transactions whose body sizes did not match the advertised sizes: {mismatches:?}")
            }
            ProtocolError::AckedTooManyTxids => {
                write!(f, "peer acknowledged more txids than are currently outstanding")
            }
            ProtocolError::RequestedNothing => write!(f, "peer requested zero txids"),
            ProtocolError::RequestedTooManyTxIds { req, unacked, max_unacked } => {
                write!(
                    f,
                    "peer requested {req} txids which would put the total in flight ({unacked} unacked) over the cap of {max_unacked}"
                )
            }
            ProtocolError::RequestBlocking => write!(
                f,
                "peer made a blocking RequestTxIds while txs were still unacknowledged; should have used non-blocking"
            ),
            ProtocolError::RequestNonBlocking => {
                write!(f, "peer made a non-blocking RequestTxIds with no unacknowledged txs; should have used blocking")
            }
            ProtocolError::RequestedUnavailableTx => {
                write!(f, "peer requested a transaction that is not available")
            }
            ProtocolError::TxIdsEmptyInBlockingReply => {
                write!(f, "peer sent no txids in response to a blocking request")
            }
        }
    }
}

impl Display for TxSizeMismatch {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let TxSizeMismatch { tx_id, advertised, actual } = self;
        write!(f, "tx {tx_id} body size {actual} does not match advertised size {advertised}")
    }
}

impl From<ProtocolError> for TerminationCause {
    fn from(err: ProtocolError) -> Self {
        TerminationCause::Protocol(err)
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
            Outcome::Error(err) => write!(f, "TerminationCause({err})"),
            Outcome::Initiator(msg) => write!(f, "Initiator({msg})"),
            Outcome::Responder(msg) => write!(f, "Responder({msg})"),
        }
    }
}
