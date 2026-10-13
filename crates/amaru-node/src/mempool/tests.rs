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
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

use amaru_kernel::{Point, Transaction, to_cbor};
use amaru_mempool::inspection::Generation;
use amaru_ouroboros::{MempoolMsg, TxInsertResult};
use amaru_ouroboros_traits::{TransactionValidationError, TxRejectReason};
use parking_lot::Mutex;
use tokio::{sync::Notify, time::timeout};

use super::{MempoolError, MempoolEvent, Pool};
use crate::tests::{setup::make_mempool_runtime, test_data::create_transaction};

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
    let (runtime, running) = make_mempool_runtime(Arc::new(Pool::default()), validator);
    let mempool = runtime.services();
    let first = create_transaction(0);
    let second = create_transaction(1);
    mempool.submit(&to_cbor(&first)).await.unwrap();
    mempool.submit(&to_cbor(&second)).await.unwrap();
    let (snapshot, mut receiver) = mempool.subscribe().unwrap();
    reject.store(true, Ordering::Release);
    runtime.sender.send(MempoolMsg::NewTip(Point::Origin)).await.unwrap();
    let event = timeout(Duration::from_secs(2), receiver.recv()).await.unwrap().unwrap();
    assert!(
        matches!(event, MempoolEvent::Removed { generation, transaction_ids } if generation == snapshot.generation.next() && transaction_ids == [first.tx_id(), second.tx_id()])
    );
    let empty = mempool.snapshot().unwrap();
    assert_eq!(empty.generation, snapshot.generation.next());
    assert_eq!(empty.transaction_count, 0);
    assert_eq!(empty.total_size_bytes, 0);
    assert!(matches!(
        mempool.submit(&to_cbor(&first)).await,
        Ok(TxInsertResult::Rejected { reason: TxRejectReason::Invalid(_), .. })
    ));
    assert_eq!(mempool.snapshot().unwrap().generation, empty.generation);
    runtime.close();
    running.request_abort();
    assert!(running.join().await.unwrap().unexpected_exits.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_wakes_pending_calls_and_subscriptions_without_retaining_pool() {
    let entered = Arc::new(Notify::new());
    let validator_entered = entered.clone();
    let (release, gate) = mpsc::channel();
    let gate = Mutex::new(gate);
    let validator = Arc::new(move |_tx: &Transaction| {
        validator_entered.notify_one();
        gate.lock().recv_timeout(Duration::from_secs(5)).unwrap();
        Ok(())
    });
    let (runtime, running) = make_mempool_runtime(Arc::new(Pool::default()), validator);
    let pool = Arc::downgrade(&runtime.pool);
    let mempool = runtime.services();
    let (_, mut receiver) = mempool.subscribe().unwrap();
    let observer = tokio::spawn(async move { receiver.recv().await });
    let submitter = mempool.clone();
    let request = tokio::spawn(async move { submitter.submit(&to_cbor(&create_transaction(0))).await });
    timeout(Duration::from_secs(2), entered.notified()).await.unwrap();
    runtime.close();
    let outcome = timeout(Duration::from_secs(2), request).await.unwrap().unwrap();
    running.request_abort();
    release.send(()).unwrap();
    assert_eq!(outcome, Err(MempoolError::Closed));
    assert_eq!(timeout(Duration::from_secs(2), observer).await.unwrap().unwrap(), Err(MempoolError::Closed));
    assert!(running.join().await.unwrap().unexpected_exits.is_empty());
    drop(runtime);
    assert!(pool.upgrade().is_none());
    assert_eq!(mempool.snapshot().unwrap_err(), MempoolError::Closed);
    assert_eq!(mempool.submit(&[]).await.unwrap_err(), MempoolError::Closed);
}
