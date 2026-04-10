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

use amaru_kernel::Transaction;
use amaru_ouroboros_traits::{MempoolError, TxId, TxInsertResult, TxOrigin, TxRejectReason, TxSubmissionMempool};
use amaru_protocols::{
    mempool_effects::MemoryPool,
    tx_submission::{MempoolInsertError, MempoolMsg},
};
use pure_stage::Effects;

use crate::effects::{Ledger, LedgerOps};

/// Create a stage that accepts messages to validate then insert transactions into the mempool.
/// The mempool messages contain a caller reference that is used to return insertion results.
///
pub async fn stage(state: MempoolStageState, msg: MempoolMsg, eff: Effects<MempoolMsg>) -> MempoolStageState {
    let memory_pool = MemoryPool::new(eff.clone());
    let ledger = Ledger::new(eff.clone());
    match msg {
        MempoolMsg::Insert { tx, origin, caller } => {
            let tx = *tx;
            let tx_id = TxId::from(&tx);
            match validate_and_insert(&ledger, &memory_pool, tx, &origin) {
                Ok(result) => eff.send(&caller, Ok(result)).await,
                Err(error) => {
                    tracing::error!(%error, %tx_id, "failed to insert transaction into mempool");
                    eff.send(&caller, Err(MempoolInsertError { tx_id, error })).await;
                }
            };
        }
        MempoolMsg::InsertBatch { txs, origin, caller } => {
            let mut results = Vec::with_capacity(txs.len());
            for tx in txs {
                let tx_id = TxId::from(&tx);
                match validate_and_insert(&ledger, &memory_pool, tx, &origin) {
                    Ok(result) => results.push(result),
                    Err(error) => {
                        tracing::error!(%error, %tx_id, "failed to insert transaction into mempool");
                        eff.send(&caller, Err(MempoolInsertError { tx_id, error })).await;
                        return state;
                    }
                }
            }
            eff.send(&caller, Ok(results)).await;
        }
    }
    state
}

/// Validate a transaction against the current ledger state
/// and insert it into the mempool if it is valid.
///
fn validate_and_insert(
    ledger: &Ledger<MempoolMsg>,
    memory_pool: &MemoryPool<MempoolMsg>,
    tx: Transaction,
    origin: &TxOrigin,
) -> Result<TxInsertResult, MempoolError> {
    match ledger.validate_tx(&tx) {
        Ok(()) => memory_pool.insert(tx, origin.clone()),
        Err(error) => Ok(TxInsertResult::rejected(TxId::from(&tx), TxRejectReason::Invalid(error))),
    }
}

pub type MempoolStageState = ();
