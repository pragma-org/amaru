// Copyright 2026 PRAGMA
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
    fmt,
    fmt::{Display, Formatter},
};

use amaru_kernel::{Peer, TxId};
use serde::{Deserialize, Serialize};

/// Origin of a transaction being inserted into the mempool:
/// - Local: created locally
/// - Remote(Peer): received from a remote peer
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum TxOrigin {
    Local,
    Remote(Peer),
}

/// Sequence number assigned to a transaction when inserted into the mempool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub struct MempoolSeqNo(pub u64);

impl MempoolSeqNo {
    pub fn next(&self) -> MempoolSeqNo {
        MempoolSeqNo(self.0 + 1)
    }

    pub fn add(&self, n: u64) -> MempoolSeqNo {
        MempoolSeqNo(self.0 + n)
    }
}

impl Display for MempoolSeqNo {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TxInsertResult {
    Accepted { tx_id: TxId, seq_no: MempoolSeqNo },
    Rejected { tx_id: TxId, reason: TxRejectReason },
}

impl TxInsertResult {
    pub fn accepted(tx_id: TxId, seq_no: MempoolSeqNo) -> Self {
        Self::Accepted { tx_id, seq_no }
    }

    pub fn rejected(tx_id: TxId, reason: TxRejectReason) -> Self {
        Self::Rejected { tx_id, reason }
    }

    pub fn tx_id(&self) -> &TxId {
        match self {
            Self::Accepted { tx_id, .. } => tx_id,
            Self::Rejected { tx_id, .. } => tx_id,
        }
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
pub enum TxRejectReason {
    #[error("Mempool is full")]
    MempoolFull,
    #[error("Transaction is a duplicate")]
    Duplicate,
    #[error(transparent)]
    Invalid(#[from] TransactionValidationError),
}

#[derive(Debug, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
pub struct TransactionValidationError(String);

impl Display for TransactionValidationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<anyhow::Error> for TransactionValidationError {
    fn from(error: anyhow::Error) -> Self {
        TransactionValidationError(error.to_string())
    }
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[error("MempoolError: {message}")]
pub struct MempoolError {
    message: String,
}

impl MempoolError {
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into() }
    }
}

impl From<anyhow::Error> for MempoolError {
    fn from(error: anyhow::Error) -> Self {
        Self::new(error.to_string())
    }
}
