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
                Err(e) => {
                    tracing::error!(error = %e, %tx_id, "failed to insert transaction into mempool");
                    eff.send(&caller, Err(MempoolInsertError { tx_id, error: e })).await;
                }
            };
        }
        MempoolMsg::InsertBatch { txs, origin, caller } => {
            let mut results = Vec::with_capacity(txs.len());
            for tx in txs {
                let tx_id = TxId::from(&tx);
                match validate_and_insert(&ledger, &memory_pool, tx, &origin) {
                    Ok(result) => results.push(result),
                    Err(e) => {
                        tracing::error!(error = %e, %tx_id, "failed to insert transaction into mempool");
                        eff.send(&caller, Err(MempoolInsertError { tx_id, error: e })).await;
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use amaru_kernel::{Hash, Transaction, TransactionBody, TransactionInput, WitnessSet, size::TRANSACTION_BODY};
    use amaru_mempool::InMemoryMempool;
    use amaru_ouroboros::ResourceMempool;
    use amaru_ouroboros_traits::{TransactionValidationError, TxOrigin};
    use amaru_protocols::tx_submission::MEMPOOL_INSERT_TIMEOUT;
    use pure_stage::{Sender, StageGraph, tokio::TokioBuilder};
    use tokio::runtime::Handle;

    use super::*;
    use crate::effects::ResourceTxValidation;

    #[tokio::test]
    async fn insert_batch_returns_one_result_per_transaction() {
        let sender = make_mempool_sender(Arc::new(InMemoryMempool::<Transaction>::default()), Arc::new(reject_tx_1));
        let tx_0 = create_transaction(0);
        let tx_1 = create_transaction(1);
        // one ok transaction
        // one invalid
        // one duplicated
        let txs = vec![tx_0.clone(), tx_1.clone(), tx_0.clone()];
        let batch = txs.clone();

        let result = sender
            .call(
                move |caller| MempoolMsg::InsertBatch { txs: batch.clone(), origin: TxOrigin::Local, caller },
                MEMPOOL_INSERT_TIMEOUT,
            )
            .await;

        let Ok(Ok(results)) = result else { panic!("batch insert should succeed") };

        assert_eq!(results.len(), txs.len());
        assert!(matches!(results[0], TxInsertResult::Accepted { .. }));
        assert!(matches!(results[1], TxInsertResult::Rejected { .. }));
        assert!(matches!(results[2], TxInsertResult::Rejected { .. }));
    }

    // HELPERS

    /// Make a sender to be able to call the mempool stage with messages
    fn make_mempool_sender(
        mempool: ResourceMempool<Transaction>,
        validator: ResourceTxValidation,
    ) -> Sender<MempoolMsg> {
        let mut graph = TokioBuilder::default();
        let mempool_stage = graph.stage("mempool", stage);
        let mempool_stage = graph.wire_up(mempool_stage, ());
        graph.resources().put::<ResourceMempool<Transaction>>(mempool);
        graph.resources().put::<ResourceTxValidation>(validator);
        let sender = graph.input(mempool_stage.without_state());
        let _ = graph.run(Handle::current());
        sender
    }

    /// This implementation of the CanValidateTxs trait rejects the transaction with input index 1
    fn reject_tx_1(tx: &Transaction) -> Result<(), TransactionValidationError> {
        if tx.body.inputs.first().is_some_and(|input| input.index == 1) {
            Err(anyhow::anyhow!("transaction rejected for testing").into())
        } else {
            Ok(())
        }
    }

    /// Create a transaction with an input at a specific index
    fn create_transaction(input_index: usize) -> Transaction {
        let tx_input = TransactionInput { transaction_id: Hash::new([1; TRANSACTION_BODY]), index: input_index as u64 };
        let body = TransactionBody::new([tx_input], [], 0);
        Transaction { body, witnesses: WitnessSet::default(), is_expected_valid: true, auxiliary_data: None }
    }
}
