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

use amaru_kernel::{Transaction, cbor::WithOriginalBytes, to_cbor};
use amaru_mempool::InMemoryMempool;
use amaru_metrics::mempool::{MempoolMetricEvent, MempoolMetrics, TxInsertionOrigin, TxInsertionResult};
use amaru_observability::tracing::Level;
use amaru_ouroboros::{
    MempoolMsg, MempoolSeqNo, MempoolState, TransactionValidationError, TxInsertResult, TxOrigin, TxRejectReason,
};
use amaru_ouroboros_traits::{TxSubmissionMempool, in_memory_chain_store::InMemoryChainStore};
use amaru_pure_stage::StageRef;

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

    logs.assert_and_remove(Level::INFO, &["transaction.accepted"])
        .assert_and_remove(Level::INFO, &["transaction.rejected", "invalid", "transaction rejected for testing"])
        .assert_and_remove(Level::INFO, &["transaction.rejected", "duplicate"])
        .assert_and_remove(Level::DEBUG, &["state.update", "tx_count=1", "size_bytes=51"])
        .assert_and_remove(Level::DEBUG, &["transaction.received", r#"origin="local""#])
        .assert_and_remove(Level::DEBUG, &["transaction.received", r#"origin="local""#])
        .assert_and_remove(Level::DEBUG, &["transaction.received", r#"origin="local""#])
        .assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn new_tip_invalidates_transactions_against_current_ledger_state() {
    let tx_0 = create_transaction(0);
    let tx_1 = create_transaction(1);
    let tx_2 = create_transaction(2);
    let mempool = Arc::new(InMemoryMempool::<WithOriginalBytes<Transaction>>::default());
    mempool.insert(tx_0.clone(), TxOrigin::Local);
    mempool.insert(tx_1.clone(), TxOrigin::Local);
    mempool.insert(tx_2.clone(), TxOrigin::Local);
    let prep = TestPrep {
        msg: MempoolMsg::NewTip(amaru_kernel::Point::Origin),
        rt: crate::stages::test_utils::test_runtime(),
        mempool: mempool.clone(),
        validator: Arc::new(reject_tx_1),
        chain_store: Arc::new(InMemoryChainStore::default()),
    };

    let (_running, _guards, mut logs) = setup(&prep);

    assert_eq!(mempool.mempool_txs(), vec![tx_0, tx_2]);
    logs.assert_and_remove(Level::INFO, &["transaction.evicted", "evicted_after_new_tip"]);
    logs.assert_and_remove(Level::DEBUG, &["transaction.revalidation_detail", "total_before=3", "evicted_count=1"]);
    logs.assert_and_remove(Level::DEBUG, &["state.update", "tx_count=2", "size_bytes=102"]);
    logs.assert_no_remaining_at([Level::DEBUG, Level::INFO, Level::WARN, Level::ERROR]);
}

#[test]
fn new_tip_reports_transactions_included_in_the_adopted_block() {
    use amaru_kernel::{IsHeader, cardano::network_block::EncodedTestBlock, cbor, make_header};
    use amaru_ouroboros_traits::{WriteChainStore, in_memory_chain_store::InMemoryChainStore};

    let encoded = EncodedTestBlock::from_seed(&make_header(1, 1, None), &amaru_kernel::EraHistory::default());
    let header = encoded.header;
    let raw = encoded.raw;
    let (_, block): (u16, amaru_kernel::Block) = cbor::decode(&raw).unwrap();

    // The mempool holds the transaction carried by the adopted block (identified by its body
    // alone) and an unrelated one, both invalidated by the new tip.
    let included_tx: WithOriginalBytes<Transaction> = Transaction {
        body: block.transaction_bodies[0].clone(),
        witnesses: Default::default(),
        is_expected_valid: true,
        auxiliary_data: None,
    }
    .into();
    let evicted_tx = create_transaction(0);
    let mempool = Arc::new(InMemoryMempool::<WithOriginalBytes<Transaction>>::default());
    mempool.insert(included_tx.clone(), TxOrigin::Local);
    mempool.insert(evicted_tx.clone(), TxOrigin::Local);

    let chain_store = Arc::new(InMemoryChainStore::default());
    chain_store.store_block(&header.hash(), &raw).unwrap();

    let prep = TestPrep {
        msg: MempoolMsg::NewTip(amaru_kernel::Point::Specific(1.into(), header.hash(), 1.into())),
        rt: crate::stages::test_utils::test_runtime(),
        mempool: mempool.clone(),
        validator: Arc::new(|_: &Transaction| Err(anyhow::anyhow!("invalid after tip").into())),
        chain_store,
    };

    let (_running, _guards, mut logs) = setup(&prep);

    assert!(mempool.mempool_txs().is_empty());
    logs.assert_and_remove(Level::INFO, &["transaction.evicted", "included_in_adopted_block"])
        .assert_and_remove(Level::INFO, &["transaction.evicted", "evicted_after_new_tip"])
        .assert_no_remaining_at([Level::INFO, Level::WARN, Level::ERROR]);
}

pub fn make_insert_batch_example() -> TestPrep {
    let caller = StageRef::named_for_tests("caller");
    let tx_0 = create_transaction(0);
    let tx_1 = create_transaction(1);
    let txs = vec![tx_0.clone(), tx_1.clone(), tx_0.clone()];

    TestPrep {
        msg: MempoolMsg::InsertBatch { txs, origin: TxOrigin::Local, caller },
        rt: crate::stages::test_utils::test_runtime(),
        mempool: Arc::new(InMemoryMempool::<WithOriginalBytes<Transaction>>::default()),
        validator: Arc::new(reject_tx_1),
        chain_store: Arc::new(InMemoryChainStore::default()),
    }
}

/// Return a transaction as invalid if its index is 1
fn reject_tx_1(tx: &Transaction) -> Result<(), TransactionValidationError> {
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

fn expected_results(txs: &[WithOriginalBytes<Transaction>]) -> Vec<TxInsertResult> {
    vec![
        TxInsertResult::accepted(txs[0].tx_id(), MempoolSeqNo(1)),
        TxInsertResult::rejected(
            txs[1].tx_id(),
            TxRejectReason::Invalid(anyhow::anyhow!("transaction rejected for testing").into()),
        ),
        TxInsertResult::rejected(txs[2].tx_id(), TxRejectReason::Duplicate),
    ]
}
