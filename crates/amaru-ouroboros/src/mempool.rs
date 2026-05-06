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

use amaru_kernel::{Tip, Transaction};
use amaru_ouroboros_traits::{MempoolError, MempoolSeqNo, TxId, TxInsertResult, TxOrigin, TxRejectReason};
use pure_stage::StageRef;

/// Messages accepted by the mempool stage.
///
/// `Insert` is used for single-transaction submission (e.g. the HTTP Submit API).
///
/// `InsertBatch` is used for bulk insertion from the TX submission protocol,
/// where transactions arrive in batches and a single round-trip is preferable.
///
/// The response to `InsertBatch` contains one `TxInsertResult` per input transaction,
/// in the same order.
///
/// NewTip comes from the `adopt_chain` stage and informs the mempool that a new tip has been adopted.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum MempoolMsg {
    WaitForAtLeast { seq_no: MempoolSeqNo, caller: StageRef<()> },
    Insert { tx: Box<Transaction>, origin: TxOrigin, caller: StageRef<Result<TxInsertResult, MempoolError>> },
    InsertBatch { txs: Vec<Transaction>, origin: TxOrigin, caller: StageRef<Result<Vec<TxInsertResult>, MempoolError>> },
    NewTip(Tip),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MempoolInsertResult {
    pub tx_id: TxId,
    pub rejected: Option<TxRejectReason>,
}
