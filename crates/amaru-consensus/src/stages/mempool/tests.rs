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

use std::sync::Arc;

use amaru_kernel::{Slot, Transaction, to_cbor};
use amaru_mempool::InMemoryMempool;
use amaru_metrics::mempool::{MempoolMetricEvent, MempoolMetrics, TxInsertionOrigin, TxInsertionResult};
use amaru_ouroboros::{
    MempoolMsg, MempoolSeqNo, MempoolState, TransactionValidationError, TxId, TxInsertResult, TxOrigin, TxRejectReason,
};
use amaru_ouroboros_traits::{MempoolError, TxSubmissionMempool};
use pure_stage::StageRef;
use tokio::runtime::Builder;
use tracing::Level;

use crate::stages::{
    mempool::{
        MempoolStageState,
        test_setup::{
            TestPrep, create_transaction, setup, te_insert, te_mempool_state, te_record_metrics, te_send,
            te_validate_tx,
        },
    },
    test_utils::{assert_trace, te_input, te_state},
};

#[test]
fn insert_batch_returns_one_result_per_transaction() {
    let batch_example = make_insert_batch_example();
    let expected_msg = batch_example.msg.clone();
    let (running, _guards, mut logs) = setup(&batch_example);

    let MempoolMsg::InsertBatch { txs, .. } = batch_example.msg else { unreachable!() };
    // After tx[0] is accepted the mempool holds exactly one transaction; tx[1] is rejected by the
    // validator and tx[2] is a duplicate of tx[0], so neither changes the state.
    let state = MempoolState { size_bytes: to_cbor(&txs[0]).len() as u64, tx_count: 1 };
    assert_trace(
        &running,
        &[
            te_state("mempool-1", &MempoolStageState::default()),
            te_input("mempool-1", &expected_msg),
            te_validate_tx("mempool-1", &txs[0]),
            te_insert("mempool-1", &txs[0], TxOrigin::Local),
            te_mempool_state("mempool-1"),
            te_record_metrics("mempool-1", insertion_metric(state, TxInsertionResult::Accepted)),
            te_validate_tx("mempool-1", &txs[1]),
            te_mempool_state("mempool-1"),
            te_record_metrics("mempool-1", insertion_metric(state, TxInsertionResult::RejectedInvalid)),
            te_validate_tx("mempool-1", &txs[2]),
            // Note that the de-duplication check is performed by the mempool when the insertion
            // is attempted
            te_insert("mempool-1", &txs[2], TxOrigin::Local),
            te_mempool_state("mempool-1"),
            te_record_metrics("mempool-1", insertion_metric(state, TxInsertionResult::RejectedDuplicate)),
            te_send("mempool-1", "caller", expected_results(&txs)),
            te_state("mempool-1", &MempoolStageState::default()),
        ],
    );

    logs.assert_and_remove(Level::INFO, &["transaction rejected by mempool", "transaction rejected for testing"])
        .assert_and_remove(Level::INFO, &["transaction rejected by mempool", "Transaction is a duplicate"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn new_tip_invalidates_transactions_against_current_ledger_state() {
    let tx_0 = create_transaction(0);
    let tx_1 = create_transaction(1);
    let tx_2 = create_transaction(2);
    let mempool = Arc::new(InMemoryMempool::<Transaction>::default());
    mempool.insert(tx_0.clone(), TxOrigin::Local).unwrap();
    mempool.insert(tx_1.clone(), TxOrigin::Local).unwrap();
    mempool.insert(tx_2.clone(), TxOrigin::Local).unwrap();
    let prep = TestPrep {
        msg: MempoolMsg::NewTip(amaru_kernel::Tip::origin()),
        rt: Builder::new_current_thread().build().unwrap(),
        mempool: mempool.clone(),
        validator: Arc::new(reject_tx_1),
    };

    let (_running, _guards, mut logs) = setup(&prep);

    assert_eq!(mempool.mempool_txs(), vec![tx_0, tx_2]);
    logs.assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

pub fn make_insert_batch_example() -> TestPrep {
    let caller = StageRef::named_for_tests("caller");
    let tx_0 = create_transaction(0);
    let tx_1 = create_transaction(1);
    let txs = vec![tx_0.clone(), tx_1.clone(), tx_0.clone()];

    TestPrep {
        msg: MempoolMsg::InsertBatch { txs, origin: TxOrigin::Local, caller },
        rt: Builder::new_current_thread().build().unwrap(),
        mempool: Arc::new(InMemoryMempool::<Transaction>::default()),
        validator: Arc::new(reject_tx_1),
    }
}

/// Return a transaction as invalid if its index is 1
fn reject_tx_1(tx: &Transaction, _slot: Slot) -> Result<(), TransactionValidationError> {
    if tx.body.inputs.first().is_some_and(|input| input.index == 1) {
        Err(anyhow::anyhow!("transaction rejected for testing").into())
    } else {
        Ok(())
    }
}

fn insertion_metric(state: MempoolState, result: TxInsertionResult) -> MempoolMetrics {
    MempoolMetrics {
        size_bytes: state.size_bytes,
        tx_count: state.tx_count,
        event: MempoolMetricEvent::TxInsertion { origin: TxInsertionOrigin::Local, result },
    }
}

fn expected_results(txs: &[Transaction]) -> Result<Vec<TxInsertResult>, MempoolError> {
    Ok(vec![
        TxInsertResult::accepted(TxId::from(&txs[0]), MempoolSeqNo(1)),
        TxInsertResult::rejected(
            TxId::from(&txs[1]),
            TxRejectReason::Invalid(anyhow::anyhow!("transaction rejected for testing").into()),
        ),
        TxInsertResult::rejected(TxId::from(&txs[2]), TxRejectReason::Duplicate),
    ])
}
