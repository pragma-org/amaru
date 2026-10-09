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

use std::sync::mpsc;

use amaru_consensus::{
    effects::{ResourceBlockValidation, ResourceEraHistory, ResourceTxValidation},
    stages::mempool::{MempoolStageState, stage},
};
use amaru_kernel::{NonEmptyVec, Peer, PlutusScript, Point, WitnessSet, cbor::WithSize, to_cbor};
use amaru_mempool::MempoolConfig;
use amaru_ouroboros::{ResourceMempool, in_memory_chain_store::InMemoryChainStore};
use amaru_ouroboros_traits::{MockBlockValidator, MockCanValidateTxs, TransactionValidationError};
use amaru_protocols::store_effects::{ResourceHeaderStore, ResourceParameters};
use amaru_pure_stage::{
    StageGraph, StageGraphRunning,
    tokio::{TokioBuilder, TokioRunning},
};
use parking_lot::Mutex;
use tokio::{runtime::Handle, sync::Notify, time::timeout};

use super::*;
use crate::{Config, tests::test_data::create_transaction};

fn test_runtime(config: MempoolConfig, validator: ResourceTxValidation) -> (Arc<MempoolRuntime>, TokioRunning) {
    let node_config = Config::default();
    let mut builder = TokioBuilder::default().with_global_epoch_offset(node_config.compute_global_clock_offset());
    let mempool_stage = builder.stage("mempool", stage);
    let mempool_stage = builder.wire_up(mempool_stage, MempoolStageState::default());
    let pool = Arc::new(Pool::new(config));
    builder.resources().put::<ResourceParameters>(node_config.global_parameters().clone());
    builder.resources().put::<ResourceEraHistory>(node_config.era_history().clone());
    builder.resources().put::<ResourceBlockValidation>(Arc::new(MockBlockValidator::default()));
    builder.resources().put::<ResourceHeaderStore>(Arc::new(InMemoryChainStore::default()));
    builder.resources().put::<ResourceMempool<Transaction>>(pool.clone());
    builder.resources().put::<ResourceTxValidation>(validator);
    let sender = builder.input(mempool_stage.without_state());
    let running = builder.run(Handle::current());
    let runtime = MempoolRuntime::new(NodeRunId::new().unwrap(), pool, sender, running.termination());
    (runtime, running)
}

fn original_bytes(tx: &WithOriginalBytes<Transaction>) -> Vec<u8> {
    let canonical = to_cbor(tx);
    assert_eq!(canonical[0], 0x84);
    [&[0x98, 0x04][..], &canonical[1..]].concat()
}

fn transaction_bytes_of_size(id: u16, size: usize) -> Vec<u8> {
    let tx = create_transaction(id);
    let encode = |script_size| {
        let witnesses = WitnessSet {
            plutus_v1_script: Some(NonEmptyVec::singleton(PlutusScript(vec![0; script_size].into()))),
            ..WitnessSet::default()
        };
        to_cbor(&Transaction {
            body: tx.body.clone(),
            witnesses: WithSize::new(witnesses, 0),
            is_expected_valid: true,
            auxiliary_data: None,
        })
    };
    let initial_script_size = size / 2;
    let overhead = encode(initial_script_size).len() - initial_script_size;
    let bytes = encode(size - overhead);
    assert_eq!(bytes.len(), size);
    bytes
}

#[tokio::test]
async fn oversized_input_is_rejected_before_cbor_decoding() {
    let (runtime, running) = test_runtime(MempoolConfig::default(), Arc::new(MockCanValidateTxs));
    let submitter = runtime.submitter();
    let max_bytes = MempoolSubmitter::MAX_INPUT_SIZE_BYTES;
    for size in [0, max_bytes - 1, max_bytes, max_bytes + 1] {
        let bytes = vec![0xff; size];
        for result in [submitter.submit(&bytes).await, submitter.submit_with_timeout(&bytes, Duration::ZERO).await] {
            if size > max_bytes {
                assert_eq!(result, Err(MempoolSubmitError::InputTooLarge { size_bytes: size, max_bytes }));
            } else {
                assert!(matches!(result, Err(MempoolSubmitError::InvalidCbor { .. })));
            }
        }
    }
    assert_eq!(runtime.reader().snapshot().unwrap().generation, 0);
    runtime.close();
    running.request_abort();
    assert!(running.join().await.unwrap().unexpected_exits.is_empty());
}

#[tokio::test]
async fn input_ceiling_allows_boundary_transactions_and_preserves_ledger_rejections() {
    let (runtime, running) = test_runtime(MempoolConfig::default(), Arc::new(MockCanValidateTxs));
    let submitter = runtime.submitter();
    let max_bytes = MempoolSubmitter::MAX_INPUT_SIZE_BYTES;
    for (id, size) in [(0, max_bytes - 1), (1, max_bytes)] {
        let bytes = transaction_bytes_of_size(id, size);
        submitter.submit(&bytes).await.unwrap();
    }
    let snapshot = runtime.reader().snapshot().unwrap();
    assert_eq!(snapshot.transaction_count, 2);
    let oversized = transaction_bytes_of_size(2, max_bytes + 1);
    assert_eq!(
        submitter.submit(&oversized).await,
        Err(MempoolSubmitError::InputTooLarge { size_bytes: oversized.len(), max_bytes })
    );
    assert_eq!(runtime.reader().snapshot().unwrap(), snapshot);
    runtime.close();
    running.request_abort();
    assert!(running.join().await.unwrap().unexpected_exits.is_empty());

    let validator = Arc::new(|_tx: &Transaction| {
        Err(TransactionValidationError::from(anyhow::anyhow!("transaction exceeds ledger size limit")))
    });
    let (runtime, running) = test_runtime(MempoolConfig::default(), validator);
    let bytes = transaction_bytes_of_size(0, max_bytes);
    assert!(matches!(
        runtime.submitter().submit(&bytes).await,
        Err(MempoolSubmitError::Rejected { reason: TxRejectReason::Invalid(_), .. })
    ));
    assert_eq!(runtime.reader().snapshot().unwrap().generation, 0);
    runtime.close();
    running.request_abort();
    assert!(running.join().await.unwrap().unexpected_exits.is_empty());
}

#[tokio::test]
async fn snapshots_and_subscriptions_include_local_and_remote_original_encodings() {
    let (runtime, running) = test_runtime(MempoolConfig::default(), Arc::new(MockCanValidateTxs));
    let mempool = runtime.services();
    let reader = mempool.reader();
    let first = create_transaction(0);
    let bytes = original_bytes(&first);
    let submitter = mempool.submitter();
    let accepted = submitter.submit(&bytes).await.unwrap();
    assert_eq!(accepted.transaction_id, first.tx_id());
    let (snapshot, mut receiver) = reader.subscribe().unwrap();
    assert_eq!(snapshot.generation, 1);
    assert_eq!(snapshot.entries[0].original_bytes, bytes);
    assert_eq!(snapshot.entries[0].origin, TxOrigin::Local);
    assert_eq!(snapshot.entries[0].sequence, accepted.sequence);

    let second = create_transaction(1);
    let remote_tx = second.clone();
    let peer = Peer::for_test(3005);
    let remote = runtime
        .sender
        .call(
            move |caller| MempoolMsg::InsertBatch { txs: vec![remote_tx], origin: TxOrigin::Remote(peer), caller },
            Duration::from_secs(2),
        )
        .await
        .unwrap();
    assert!(matches!(remote[0], TxInsertResult::Accepted { .. }));
    let event = receiver.recv().await.unwrap();
    assert!(matches!(&event, MempoolEvent::Inserted { run_id, generation: 2, entry }
        if *run_id == snapshot.run_id && entry.transaction_id == second.tx_id() && entry.origin == TxOrigin::Remote(peer) && entry.original_bytes == to_cbor(&second)));
    let current = mempool.reader().snapshot().unwrap();
    assert!(matches!(&event, MempoolEvent::Inserted { entry, .. } if entry == &current.entries[1]));
    assert_eq!(current.transaction_count, 2);
    assert_eq!(
        current.entries.iter().map(|entry| entry.transaction_id).collect::<Vec<_>>(),
        [first.tx_id(), second.tx_id()]
    );
    assert_eq!(current.total_size_bytes, current.entries.iter().map(|entry| entry.size_bytes).sum::<u64>());

    assert!(
        matches!(submitter.submit(&bytes).await, Err(MempoolSubmitError::Rejected { transaction_id, reason: TxRejectReason::Duplicate }) if transaction_id == first.tx_id())
    );
    assert_eq!(reader.snapshot().unwrap(), current);
    runtime.close();
    running.request_abort();
    assert!(running.join().await.unwrap().unexpected_exits.is_empty());
}

#[tokio::test]
async fn revalidation_emits_one_complete_removal_generation() {
    let reject = Arc::new(AtomicBool::new(false));
    let reject_validator = reject.clone();
    let validator = Arc::new(move |_tx: &Transaction| {
        if reject_validator.load(Ordering::Acquire) {
            Err(TransactionValidationError::from(anyhow::anyhow!("invalid after tip")))
        } else {
            Ok(())
        }
    });
    let (runtime, running) = test_runtime(MempoolConfig::default(), validator);
    let reader = runtime.reader();
    let submitter = runtime.submitter();
    let first = create_transaction(0);
    let second = create_transaction(1);
    submitter.submit(&to_cbor(&first)).await.unwrap();
    submitter.submit(&to_cbor(&second)).await.unwrap();
    let (snapshot, mut receiver) = reader.subscribe().unwrap();
    reject.store(true, Ordering::Release);
    runtime.sender.send(MempoolMsg::NewTip(Point::Origin)).await.unwrap();
    let event = timeout(Duration::from_secs(2), receiver.recv()).await.unwrap().unwrap();
    assert!(
        matches!(event, MempoolEvent::Removed { generation: 3, transaction_ids, .. } if transaction_ids == [first.tx_id(), second.tx_id()])
    );
    let empty = reader.snapshot().unwrap();
    assert_eq!(empty.generation, snapshot.generation + 1);
    assert_eq!(empty.transaction_count, 0);
    assert_eq!(empty.total_size_bytes, 0);
    assert!(matches!(
        submitter.submit(&to_cbor(&first)).await,
        Err(MempoolSubmitError::Rejected { reason: TxRejectReason::Invalid(_), .. })
    ));
    assert_eq!(reader.snapshot().unwrap().generation, 3);
    runtime.close();
    running.request_abort();
    assert!(running.join().await.unwrap().unexpected_exits.is_empty());
}

#[tokio::test]
async fn embedded_and_http_share_outcomes_and_original_bytes() {
    let first = create_transaction(0);
    let bytes = original_bytes(&first);
    let config = MempoolConfig::default().with_max_bytes(bytes.len() as u64);
    let (runtime, running) = test_runtime(config, Arc::new(MockCanValidateTxs));
    let submitter = runtime.submitter();
    let reader = runtime.reader();
    let (_, mut receiver) = reader.subscribe().unwrap();
    let shutdown = CancellationToken::new();
    let (server, address) =
        crate::submit_api::start("127.0.0.1:0".parse().unwrap(), submitter.clone(), shutdown.clone()).await.unwrap();
    let client = reqwest::Client::new();
    let url = format!("http://{address}/api/submit/tx");
    let response =
        client.post(&url).header("Content-Type", "application/cbor").body(bytes.clone()).send().await.unwrap();
    assert_eq!(response.status(), 202);
    assert_eq!(response.text().await.unwrap(), format!("\"{}\"", first.tx_id()));
    assert!(matches!(receiver.recv().await.unwrap(), MempoolEvent::Inserted { generation: 1, .. }));
    assert_eq!(reader.snapshot().unwrap().entries[0].original_bytes, bytes);

    for (body, status) in [
        (bytes, 409),
        (to_cbor(&create_transaction(1)), 503),
        (vec![0xde, 0xad], 400),
        (vec![0xff; MempoolSubmitter::MAX_INPUT_SIZE_BYTES + 1], 413),
    ] {
        let error = submitter.submit(&body).await.unwrap_err();
        match status {
            409 => assert!(matches!(error, MempoolSubmitError::Rejected { reason: TxRejectReason::Duplicate, .. })),
            503 => assert!(matches!(error, MempoolSubmitError::Rejected { reason: TxRejectReason::MempoolFull, .. })),
            400 => assert!(matches!(error, MempoolSubmitError::InvalidCbor { .. })),
            413 => assert!(matches!(error, MempoolSubmitError::InputTooLarge { .. })),
            _ => unreachable!(),
        }
        assert_eq!(
            client.post(&url).header("Content-Type", "application/cbor").body(body).send().await.unwrap().status(),
            status
        );
    }
    assert_eq!(reader.snapshot().unwrap().generation, 1);
    shutdown.cancel();
    server.await.unwrap();
    runtime.close();
    running.request_abort();
    assert!(running.join().await.unwrap().unexpected_exits.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn timed_out_dispatched_submission_can_still_be_accepted() {
    let entered = Arc::new(Notify::new());
    let validator_entered = entered.clone();
    let (release, gate) = mpsc::channel();
    let gate = Mutex::new(gate);
    let first_validation = AtomicBool::new(true);
    let validator = Arc::new(move |_tx: &Transaction| {
        if first_validation.swap(false, Ordering::AcqRel) {
            validator_entered.notify_one();
            gate.lock().recv_timeout(Duration::from_secs(5)).unwrap();
        }
        Ok(())
    });
    let (runtime, running) = test_runtime(MempoolConfig::default(), validator);
    let (_, mut receiver) = runtime.reader().subscribe().unwrap();
    let mempool = runtime.services();
    let tx = create_transaction(0);
    let bytes = to_cbor(&tx);
    let request =
        tokio::spawn(async move { mempool.submitter().submit_with_timeout(&bytes, Duration::from_millis(100)).await });
    timeout(Duration::from_secs(2), entered.notified()).await.unwrap();
    let result = request.await.unwrap();
    release.send(()).unwrap();
    assert_eq!(result, Err(MempoolSubmitError::Timeout { transaction_id: tx.tx_id() }));
    assert!(
        matches!(timeout(Duration::from_secs(2), receiver.recv()).await.unwrap().unwrap(), MempoolEvent::Inserted { entry, .. } if entry.transaction_id == tx.tx_id())
    );
    runtime.close();
    running.request_abort();
    assert!(running.join().await.unwrap().unexpected_exits.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_wakes_pending_submission_and_empty_subscription() {
    let entered = Arc::new(Notify::new());
    let validator_entered = entered.clone();
    let (release, gate) = mpsc::channel();
    let gate = Mutex::new(gate);
    let validator = Arc::new(move |_tx: &Transaction| {
        validator_entered.notify_one();
        gate.lock().recv_timeout(Duration::from_secs(5)).unwrap();
        Ok(())
    });
    let (runtime, running) = test_runtime(MempoolConfig::default(), validator);
    let (_, mut receiver) = runtime.reader().subscribe().unwrap();
    let observer = tokio::spawn(async move { receiver.recv().await });
    let submitter = runtime.submitter();
    let tx = create_transaction(0);
    let bytes = to_cbor(&tx);
    let request = tokio::spawn(async move { submitter.submit_with_timeout(&bytes, Duration::from_secs(30)).await });
    timeout(Duration::from_secs(2), entered.notified()).await.unwrap();
    runtime.close();
    let outcome = timeout(Duration::from_secs(2), request).await.unwrap().unwrap();
    running.request_abort();
    release.send(()).unwrap();
    assert_eq!(outcome, Err(MempoolSubmitError::Closing { transaction_id: Some(tx.tx_id()) }));
    assert_eq!(timeout(Duration::from_secs(2), observer).await.unwrap().unwrap(), Err(MempoolAccessError::Closing));
    assert!(running.join().await.unwrap().unexpected_exits.is_empty());
}

#[tokio::test]
async fn shutdown_precedes_queued_events_and_retained_handles_do_not_retain_pool() {
    let (runtime, running) = test_runtime(MempoolConfig::default(), Arc::new(MockCanValidateTxs));
    let mempool = runtime.services();
    let reader = mempool.reader();
    let submitter = mempool.submitter();
    let (snapshot, mut receiver) = reader.subscribe().unwrap();
    assert_eq!(snapshot.transaction_count, 0);
    let pool = Arc::downgrade(&runtime.pool);
    let tx = create_transaction(0);
    submitter.submit(&to_cbor(&tx)).await.unwrap();
    runtime.close();
    assert_eq!(mempool.reader().snapshot().unwrap_err(), MempoolAccessError::Closing);
    assert_eq!(
        mempool.submitter().submit(&[]).await.unwrap_err(),
        MempoolSubmitError::Closing { transaction_id: None }
    );
    assert_eq!(reader.snapshot().unwrap_err(), MempoolAccessError::Closing);
    assert_eq!(receiver.recv().await.unwrap_err(), MempoolAccessError::Closing);
    assert_eq!(submitter.submit(&[]).await.unwrap_err(), MempoolSubmitError::Closing { transaction_id: None });
    running.request_abort();
    assert!(running.join().await.unwrap().unexpected_exits.is_empty());
    drop(runtime);
    assert!(pool.upgrade().is_none());
    assert_eq!(mempool.reader().snapshot().unwrap_err(), MempoolAccessError::Stopped);
    assert_eq!(
        mempool.submitter().submit(&[]).await.unwrap_err(),
        MempoolSubmitError::Stopped { transaction_id: None }
    );
    assert_eq!(reader.snapshot().unwrap_err(), MempoolAccessError::Stopped);
    assert_eq!(receiver.recv().await.unwrap_err(), MempoolAccessError::Stopped);
    assert_eq!(submitter.submit(&[]).await.unwrap_err(), MempoolSubmitError::Stopped { transaction_id: None });

    let (next, running) = test_runtime(MempoolConfig::default(), Arc::new(MockCanValidateTxs));
    assert_ne!(snapshot.run_id, next.reader().snapshot().unwrap().run_id);
    assert_eq!(reader.snapshot().unwrap_err(), MempoolAccessError::Stopped);
    next.close();
    running.request_abort();
    assert!(running.join().await.unwrap().unexpected_exits.is_empty());
}

#[tokio::test]
async fn unexpected_graph_termination_closes_services() {
    let (runtime, running) = test_runtime(MempoolConfig::default(), Arc::new(MockCanValidateTxs));
    let (_, mut receiver) = runtime.reader().subscribe().unwrap();
    running.request_abort();
    running.termination().await;
    assert_eq!(runtime.reader().snapshot().unwrap_err(), MempoolAccessError::Closing);
    assert_eq!(receiver.recv().await.unwrap_err(), MempoolAccessError::Closing);
    assert!(running.join().await.unwrap().unexpected_exits.is_empty());
}

#[tokio::test]
async fn subscriber_gaps_identify_run_and_recovery_has_a_fresh_atomic_handoff() {
    let (runtime, running) = test_runtime(MempoolConfig::default(), Arc::new(MockCanValidateTxs));
    let reader = runtime.reader();
    let (initial, mut receiver) = reader.subscribe().unwrap();
    let txs = (0..65).map(create_transaction).collect();
    let results = runtime
        .sender
        .call(move |caller| MempoolMsg::InsertBatch { txs, origin: TxOrigin::Local, caller }, Duration::from_secs(2))
        .await
        .unwrap();
    assert!(results.iter().all(|result| matches!(result, TxInsertResult::Accepted { .. })));
    assert_eq!(
        receiver.recv().await.unwrap_err(),
        MempoolAccessError::Gap { run_id: initial.run_id, expected_generation: 1, current_generation: 65 }
    );
    let (snapshot, mut fresh) = reader.subscribe().unwrap();
    assert_eq!(snapshot.generation, 65);
    assert_eq!(snapshot.transaction_count, 65);
    runtime.submitter().submit(&to_cbor(&create_transaction(65))).await.unwrap();
    assert!(
        matches!(fresh.recv().await.unwrap(), MempoolEvent::Inserted { run_id, generation: 66, .. } if run_id == initial.run_id)
    );
    runtime.close();
    running.request_abort();
    assert!(running.join().await.unwrap().unexpected_exits.is_empty());
}
