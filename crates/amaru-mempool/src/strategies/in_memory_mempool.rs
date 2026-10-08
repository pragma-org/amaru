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
    collections::{BTreeMap, BTreeSet},
    mem,
    sync::{Arc, Weak},
    time::SystemTime,
};

use amaru_kernel::{HasTransactionId, TransactionId, cbor, to_cbor};
use amaru_ouroboros_traits::{
    MempoolSeqNo, TxInsertResult, TxOrigin, TxRejectReason, TxSubmissionMempool, mempool::Mempool,
};

use crate::inspection::{
    MAX_MEMPOOL_SUBSCRIBERS, MempoolSnapshotEntry, ObserverQueue, PoolChange, PoolObserverError, PoolReceiver,
    PoolSnapshot,
};
/// A temporary in-memory mempool implementation to support the transaction submission protocol.
///
/// It stores transactions in memory, indexed by their TransactionId and by a sequence number assigned
/// at insertion time.
///
#[derive(Clone)]
pub struct InMemoryMempool<Tx> {
    config: MempoolConfig,
    inner: Arc<parking_lot::RwLock<MempoolInner<Tx>>>,
    admission_clock: Arc<dyn Fn() -> SystemTime + Send + Sync>,
}

impl<Tx: 'static> Default for InMemoryMempool<Tx> {
    fn default() -> Self {
        Self::new(MempoolConfig::default())
    }
}

impl<Tx> InMemoryMempool<Tx> {
    /// Create a pool that records admission times using the system wall clock.
    pub fn new(config: MempoolConfig) -> Self {
        Self::new_with_admission_clock(config, SystemTime::now)
    }

    /// Create a pool with an admission clock, allowing deterministic simulation.
    ///
    /// The clock runs under the pool's write lock, only for successful new insertions.
    /// It must not block or reenter the pool.
    pub fn new_with_admission_clock(
        config: MempoolConfig,
        admission_clock: impl Fn() -> SystemTime + Send + Sync + 'static,
    ) -> Self {
        InMemoryMempool {
            config,
            inner: Arc::new(parking_lot::RwLock::new(MempoolInner::default())),
            admission_clock: Arc::new(admission_clock),
        }
    }
}

#[derive(Debug)]
struct MempoolInner<Tx> {
    next_seq: u64,
    generation: u64,
    current_bytes: u64,
    entries_by_id: BTreeMap<TransactionId, MempoolEntry<Tx>>,
    entries_by_seq: BTreeMap<MempoolSeqNo, TransactionId>,
    observers: Vec<Weak<ObserverQueue>>,
}

impl<Tx> Default for MempoolInner<Tx> {
    fn default() -> Self {
        MempoolInner {
            next_seq: 1,
            generation: 0,
            current_bytes: 0,
            entries_by_id: Default::default(),
            entries_by_seq: Default::default(),
            observers: Vec::new(),
        }
    }
}

impl<Tx> MempoolInner<Tx> {
    fn commit(&mut self, event: impl FnOnce(u64, &Self) -> PoolChange) {
        self.generation += 1;
        self.observers.retain(|observer| observer.strong_count() > 0);
        if self.observers.is_empty() {
            return;
        }
        let event = Arc::new(event(self.generation, self));
        self.observers.retain(|observer| observer.upgrade().is_some_and(|queue| queue.push(event.clone())));
    }
}

impl<Tx> Drop for MempoolInner<Tx> {
    fn drop(&mut self) {
        for queue in self.observers.iter().filter_map(Weak::upgrade) {
            queue.close();
        }
    }
}

impl<Tx: HasTransactionId + cbor::Encode<()> + Clone> MempoolInner<Tx> {
    /// Inserts a new transaction into the mempool.
    /// The transaction id is a hash of the transaction body.
    fn insert(
        &mut self,
        config: &MempoolConfig,
        admission_clock: &dyn Fn() -> SystemTime,
        tx: Tx,
        tx_origin: TxOrigin,
    ) -> Result<(TransactionId, MempoolSeqNo), TxRejectReason> {
        let tx_size = to_cbor(&tx).len() as u32;
        let tx_id = tx.tx_id();

        if self.entries_by_id.contains_key(&tx_id) {
            return Err(TxRejectReason::Duplicate);
        }

        if self.current_bytes.saturating_add(tx_size as u64) > config.max_bytes {
            return Err(TxRejectReason::MempoolFull);
        }

        let admitted_at = admission_clock();
        let seq_no = MempoolSeqNo(self.next_seq);
        self.next_seq += 1;

        let entry = MempoolEntry { seq_no, tx_id, tx, tx_size, origin: tx_origin, admitted_at };

        self.entries_by_id.insert(tx_id, entry);
        self.entries_by_seq.insert(seq_no, tx_id);
        self.current_bytes = self.current_bytes.saturating_add(tx_size as u64);
        self.commit(|generation, inner| PoolChange::Inserted {
            generation,
            entry: inner.entries_by_id[&tx_id].snapshot(),
        });
        Ok((tx_id, seq_no))
    }

    /// Retrieves a transaction by its id.
    fn get_tx(&self, tx_id: &TransactionId) -> Option<Tx> {
        self.entries_by_id.get(tx_id).map(|entry| entry.tx.clone())
    }

    /// Retrieves all the transaction ids since a given sequence number, up to a limit.
    #[expect(clippy::panic)]
    fn tx_ids_since(&self, from_seq: MempoolSeqNo, limit: u16) -> Vec<(TransactionId, u32, MempoolSeqNo)> {
        let mut result: Vec<(TransactionId, u32, MempoolSeqNo)> = self
            .entries_by_seq
            .range(from_seq..)
            .take(limit as usize)
            .map(|(seq, tx_id)| {
                let Some(entry) = self.entries_by_id.get(tx_id) else {
                    panic!("Inconsistent mempool state: entry missing for tx_id {:?}", tx_id)
                };
                (*tx_id, entry.tx_size, *seq)
            })
            .collect();
        result.sort_by_key(|(_, _, seq_no)| *seq_no);
        result
    }

    /// Retrieves transactions for the given ids, sorted by their sequence number.
    fn get_txs_for_ids(&self, ids: &[TransactionId]) -> Vec<Tx> {
        // Make sure that the result are sorted by seq_no
        let mut result: Vec<(&TransactionId, &MempoolEntry<Tx>)> =
            self.entries_by_id.iter().filter(|(key, _)| ids.contains(*key)).collect();
        result.sort_by_key(|(_, entry)| entry.seq_no);
        result.into_iter().map(|(_, entry)| entry.tx.clone()).collect()
    }

    fn mempool_txs(&self) -> Vec<Tx> {
        self.entries_by_seq
            .values()
            .filter_map(|tx_id| self.entries_by_id.get(tx_id))
            .map(|entry| entry.tx.clone())
            .collect()
    }

    fn remove_txs(&mut self, ids: &[TransactionId]) {
        let mut removed = Vec::new();
        for tx_id in ids {
            if let Some(entry) = self.entries_by_id.remove(tx_id) {
                self.entries_by_seq.remove(&entry.seq_no);
                self.current_bytes = self.current_bytes.saturating_sub(entry.tx_size as u64);
                removed.push(*tx_id);
            }
        }
        if !removed.is_empty() {
            self.commit(|generation, _| PoolChange::Removed { generation, transaction_ids: removed });
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct MempoolEntry<Tx> {
    seq_no: MempoolSeqNo,
    tx_id: TransactionId,
    tx: Tx,
    tx_size: u32,
    origin: TxOrigin,
    admitted_at: SystemTime,
}

impl<Tx: cbor::Encode<()>> MempoolEntry<Tx> {
    fn snapshot(&self) -> MempoolSnapshotEntry {
        MempoolSnapshotEntry {
            transaction_id: self.tx_id,
            sequence: self.seq_no,
            origin: self.origin.clone(),
            admitted_at: self.admitted_at,
            original_bytes: to_cbor(&self.tx),
            size_bytes: self.tx_size as u64,
        }
    }
}

impl<Tx: cbor::Encode<()>> InMemoryMempool<Tx> {
    fn snapshot_inner(&self, inner: &MempoolInner<Tx>) -> PoolSnapshot {
        PoolSnapshot {
            generation: inner.generation,
            entries: inner.entries_by_seq.values().map(|id| inner.entries_by_id[id].snapshot()).collect(),
            transaction_count: inner.entries_by_id.len() as u64,
            total_size_bytes: inner.current_bytes,
            capacity_bytes: self.config.max_bytes,
        }
    }

    /// Capture membership, insertion order, and counters in one read.
    pub fn snapshot(&self) -> PoolSnapshot {
        self.snapshot_inner(&self.inner.read())
    }

    /// Atomically capture an initial snapshot and register for all later generations.
    pub fn subscribe(&self) -> Result<(PoolSnapshot, PoolReceiver), PoolObserverError> {
        let mut inner = self.inner.write();
        inner.observers.retain(|observer| observer.strong_count() > 0);
        if inner.observers.len() >= MAX_MEMPOOL_SUBSCRIBERS {
            return Err(PoolObserverError::SubscriberLimit);
        }
        let receiver = PoolReceiver::new(inner.generation);
        let snapshot = self.snapshot_inner(&inner);
        inner.observers.push(Arc::downgrade(&receiver.queue));
        Ok((snapshot, receiver))
    }
}

#[derive(Debug, Clone)]
pub struct MempoolConfig {
    /// Maximum size on the total CBOR size of transactions held simultaneously, in bytes.
    pub max_bytes: u64,
}

/// Default mempool size: roughly twice the Conway max block body size (~90 KB).
/// This matches the 2× block-size convention used by the Cardano Haskell node.
const DEFAULT_MAX_BYTES: u64 = 180_224;

impl Default for MempoolConfig {
    fn default() -> Self {
        Self { max_bytes: DEFAULT_MAX_BYTES }
    }
}

impl MempoolConfig {
    pub fn with_max_bytes(mut self, max: u64) -> Self {
        self.max_bytes = max;
        self
    }
}

impl<Tx: Send + Sync + 'static + HasTransactionId + cbor::Encode<()> + Clone> TxSubmissionMempool<Tx>
    for InMemoryMempool<Tx>
{
    fn insert(&self, tx: Tx, tx_origin: TxOrigin) -> TxInsertResult {
        let tx_id = tx.tx_id();
        let mut inner = self.inner.write();
        let res = inner.insert(&self.config, self.admission_clock.as_ref(), tx, tx_origin);
        match res {
            Ok((tx_id, seq_no)) => TxInsertResult::accepted(tx_id, seq_no),
            Err(reason) => TxInsertResult::rejected(tx_id, reason),
        }
    }

    fn get_tx(&self, tx_id: &TransactionId) -> Option<Tx> {
        self.inner.read().get_tx(tx_id)
    }

    fn tx_ids_since(&self, from_seq: MempoolSeqNo, limit: u16) -> Vec<(TransactionId, u32, MempoolSeqNo)> {
        self.inner.read().tx_ids_since(from_seq, limit)
    }

    fn get_txs_for_ids(&self, ids: &[TransactionId]) -> Vec<Tx> {
        self.inner.read().get_txs_for_ids(ids)
    }

    fn mempool_txs(&self) -> Vec<Tx> {
        self.inner.read().mempool_txs()
    }

    fn remove_txs(&self, ids: &[TransactionId]) {
        self.inner.write().remove_txs(ids)
    }

    fn last_seq_no(&self) -> MempoolSeqNo {
        MempoolSeqNo(self.inner.read().next_seq - 1)
    }

    fn is_near_capacity(&self, additional_bytes: u64) -> bool {
        let current = self.inner.read().current_bytes;
        current.saturating_add(additional_bytes) > self.config.max_bytes
    }

    fn state(&self) -> amaru_ouroboros_traits::MempoolState {
        let inner = self.inner.read();
        amaru_ouroboros_traits::MempoolState {
            tx_count: inner.entries_by_id.len() as u64,
            size_bytes: inner.current_bytes,
        }
    }
}

impl<Tx: Send + Sync + 'static + HasTransactionId + cbor::Encode<()> + Clone> Mempool<Tx> for InMemoryMempool<Tx> {
    fn take(&self) -> Vec<Tx> {
        let mut inner = self.inner.write();
        let entries = mem::take(&mut inner.entries_by_id);
        let _ = mem::take(&mut inner.entries_by_seq);
        inner.current_bytes = 0;
        if !entries.is_empty() {
            let ids = entries.keys().copied().collect();
            inner.commit(|generation, _| PoolChange::Removed { generation, transaction_ids: ids });
        }
        entries.into_values().map(|entry| entry.tx).collect()
    }

    fn acknowledge<TxKey: Ord, I>(&self, tx: &Tx, keys: fn(&Tx) -> I)
    where
        I: IntoIterator<Item = TxKey>,
        Self: Sized,
    {
        let keys_to_remove = BTreeSet::from_iter(keys(tx));
        let mut inner = self.inner.write();

        let mut seq_nos_to_remove: Vec<MempoolSeqNo> = Vec::new();
        let mut ids_to_remove = Vec::new();
        let mut bytes_to_subtract: u64 = 0;
        for entry in inner.entries_by_id.values() {
            if keys(&entry.tx).into_iter().any(|k| keys_to_remove.contains(&k)) {
                seq_nos_to_remove.push(entry.seq_no);
                ids_to_remove.push(entry.tx_id);
                bytes_to_subtract = bytes_to_subtract.saturating_add(entry.tx_size as u64);
            }
        }
        inner.entries_by_id.retain(|_, entry| !keys(&entry.tx).into_iter().any(|k| keys_to_remove.contains(&k)));
        for seq_no in seq_nos_to_remove {
            inner.entries_by_seq.remove(&seq_no);
        }
        inner.current_bytes = inner.current_bytes.saturating_sub(bytes_to_subtract);
        if !ids_to_remove.is_empty() {
            inner.commit(|generation, _| PoolChange::Removed { generation, transaction_ids: ids_to_remove });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        ops::Deref,
        slice,
        str::FromStr,
        sync::{
            Barrier,
            atomic::{AtomicU64, Ordering},
        },
        time::Duration,
    };

    use amaru_kernel::{Hasher, Peer, cbor, cbor as minicbor, size::TRANSACTION_BODY};
    use amaru_ouroboros_traits::TxRejectReason;
    use assertables::assert_some_eq_x;

    use super::*;

    #[tokio::test]
    async fn insert_a_transaction() -> anyhow::Result<()> {
        let mempool = InMemoryMempool::new(MempoolConfig::default());
        let tx = Tx::from_str("tx1").unwrap();
        let TxInsertResult::Accepted { tx_id, seq_no: seq_nb } =
            mempool.insert(tx.clone(), TxOrigin::Remote(Peer::for_test(3005)))
        else {
            panic!("transaction should be accepted")
        };

        assert_some_eq_x!(mempool.get_tx(&tx_id), tx.clone());
        assert_eq!(mempool.get_txs_for_ids(slice::from_ref(&tx_id)), vec![tx.clone()]);
        assert_eq!(mempool.tx_ids_since(seq_nb, 100), vec![(tx_id, 5, seq_nb)]);
        assert_eq!(mempool.last_seq_no(), seq_nb);
        Ok(())
    }

    #[test]
    fn rejects_when_bytes_capacity_exceeded() {
        let first = Tx::from_str("a").unwrap();
        let max_bytes = to_cbor(&first).len() as u64;
        let mempool = InMemoryMempool::new(MempoolConfig::default().with_max_bytes(max_bytes));

        assert!(matches!(mempool.insert(first, TxOrigin::Local), TxInsertResult::Accepted { .. }));

        let TxInsertResult::Rejected { reason, .. } = mempool.insert(Tx::from_str("b").unwrap(), TxOrigin::Local)
        else {
            panic!("transaction should be rejected as full");
        };
        assert!(matches!(reason, TxRejectReason::MempoolFull), "unexpected reason: {reason:?}");
    }

    #[test]
    fn duplicate_on_full_mempool_reports_duplicate_not_full() {
        let first = Tx::from_str("a").unwrap();
        let max_bytes = to_cbor(&first).len() as u64;
        let mempool = InMemoryMempool::new(MempoolConfig::default().with_max_bytes(max_bytes));

        assert!(matches!(mempool.insert(first.clone(), TxOrigin::Local), TxInsertResult::Accepted { .. }));

        let TxInsertResult::Rejected { reason, .. } = mempool.insert(first, TxOrigin::Local) else {
            panic!("transaction should be rejected");
        };
        assert!(matches!(reason, TxRejectReason::Duplicate), "unexpected reason: {reason:?}");
    }

    #[test]
    fn remove_txs_frees_bytes_capacity() {
        let first = Tx::from_str("a").unwrap();
        let max_bytes = to_cbor(&first).len() as u64;
        let mempool = InMemoryMempool::new(MempoolConfig::default().with_max_bytes(max_bytes));

        let TxInsertResult::Accepted { tx_id, .. } = mempool.insert(first, TxOrigin::Local) else {
            panic!("first insert should succeed");
        };

        mempool.remove_txs(&[tx_id]);

        let second = Tx::from_str("b").unwrap();
        assert!(matches!(mempool.insert(second, TxOrigin::Local), TxInsertResult::Accepted { .. }));
    }

    #[tokio::test]
    async fn admission_times_survive_duplicates_and_snapshots_and_reset_on_reinsertion() {
        let first = Tx::from_str("a").unwrap();
        let clock_calls = Arc::new(AtomicU64::new(0));
        let next_time = Arc::new(parking_lot::Mutex::new(SystemTime::UNIX_EPOCH));
        let calls = clock_calls.clone();
        let time = next_time.clone();
        let config = MempoolConfig::default().with_max_bytes(to_cbor(&first).len() as u64);
        let mempool = InMemoryMempool::new_with_admission_clock(config, move || {
            calls.fetch_add(1, Ordering::Relaxed);
            *time.lock()
        });
        let (_, mut receiver) = mempool.subscribe().unwrap();
        assert!(matches!(mempool.insert(first.clone(), TxOrigin::Local), TxInsertResult::Accepted { .. }));
        let initial = mempool.snapshot().entries[0].clone();
        assert_eq!(initial.admitted_at, SystemTime::UNIX_EPOCH);
        assert!(
            matches!(receiver.recv().await.unwrap().as_ref(), PoolChange::Inserted { entry, .. } if entry == &initial)
        );

        let later = SystemTime::UNIX_EPOCH + Duration::from_secs(60);
        *next_time.lock() = later;
        let remote = TxOrigin::Remote(Peer::for_test(3005));
        assert!(matches!(
            mempool.insert(first.clone(), remote.clone()),
            TxInsertResult::Rejected { reason: TxRejectReason::Duplicate, .. }
        ));
        assert!(matches!(
            mempool.insert(Tx::from_str("b").unwrap(), TxOrigin::Local),
            TxInsertResult::Rejected { reason: TxRejectReason::MempoolFull, .. }
        ));
        assert_eq!(mempool.subscribe().unwrap().0.entries[0], initial);
        assert_eq!(clock_calls.load(Ordering::Relaxed), 1);

        mempool.remove_txs(&[initial.transaction_id]);
        receiver.recv().await.unwrap();
        assert!(matches!(mempool.insert(first, remote.clone()), TxInsertResult::Accepted { .. }));
        let readmitted = mempool.snapshot().entries[0].clone();
        assert_eq!(readmitted.admitted_at, later);
        assert_eq!(readmitted.origin, remote);
        assert!(readmitted.sequence > initial.sequence);
        assert!(
            matches!(receiver.recv().await.unwrap().as_ref(), PoolChange::Inserted { entry, .. } if entry == &readmitted)
        );

        *next_time.lock() = SystemTime::UNIX_EPOCH;
        assert_eq!(mempool.snapshot().entries[0], readmitted);
        assert_eq!(clock_calls.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn snapshot_and_changes_preserve_atomic_batches_and_noop_generations() {
        let mempool = InMemoryMempool::default();
        let first = Tx::from_str("a").unwrap();
        let second = Tx::from_str("b").unwrap();
        let first_id = first.tx_id();
        let second_id = second.tx_id();
        let origin = TxOrigin::Remote(Peer::for_test(3005));
        mempool.insert(first.clone(), origin.clone());
        let (snapshot, mut receiver) = mempool.subscribe().unwrap();
        assert_eq!(snapshot.generation, 1);
        assert_eq!(snapshot.entries[0].origin, origin);
        assert_eq!(snapshot.entries[0].original_bytes, to_cbor(&first));
        assert_eq!(snapshot.total_size_bytes, to_cbor(&first).len() as u64);

        assert!(matches!(
            mempool.insert(first, TxOrigin::Local),
            TxInsertResult::Rejected { reason: TxRejectReason::Duplicate, .. }
        ));
        assert_eq!(mempool.snapshot().generation, 1);
        mempool.insert(second, TxOrigin::Local);
        assert!(
            matches!(receiver.recv().await.unwrap().as_ref(), PoolChange::Inserted { generation: 2, entry } if entry.transaction_id == second_id)
        );

        mempool.remove_txs(&[second_id, first_id, second_id]);
        assert!(
            matches!(receiver.recv().await.unwrap().as_ref(), PoolChange::Removed { generation: 3, transaction_ids } if transaction_ids == &[second_id, first_id])
        );
        mempool.remove_txs(&[first_id]);
        let empty = mempool.snapshot();
        assert_eq!(empty.generation, 3);
        assert_eq!(empty.transaction_count, 0);
        assert_eq!(empty.total_size_bytes, 0);
        assert!(empty.entries.is_empty());
    }

    #[tokio::test]
    async fn subscription_handoff_covers_concurrent_insertion() {
        for _ in 0..32 {
            let mempool = InMemoryMempool::default();
            let tx = Tx::from_str("racing").unwrap();
            let writer = mempool.clone();
            let barrier = Arc::new(Barrier::new(2));
            let writer_barrier = barrier.clone();
            let thread = std::thread::spawn(move || {
                writer_barrier.wait();
                writer.insert(tx, TxOrigin::Local);
            });
            barrier.wait();
            let (snapshot, mut receiver) = mempool.subscribe().unwrap();
            thread.join().unwrap();
            if snapshot.generation == 0 {
                let event =
                    tokio::time::timeout(std::time::Duration::from_secs(1), receiver.recv()).await.unwrap().unwrap();
                assert_eq!(event.generation(), 1);
            } else {
                assert_eq!(snapshot.generation, 1);
                assert_eq!(snapshot.transaction_count, 1);
            }
        }
    }

    #[test]
    fn concurrent_snapshots_keep_entries_and_totals_coherent() {
        let mempool = InMemoryMempool::default();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                for i in 0..1000 {
                    let tx = Tx::from_str(&i.to_string()).unwrap();
                    let id = tx.tx_id();
                    mempool.insert(tx, TxOrigin::Local);
                    if i % 2 == 0 {
                        mempool.remove_txs(&[id]);
                    }
                }
            });
            for _ in 0..1000 {
                let snapshot = mempool.snapshot();
                assert_eq!(snapshot.transaction_count, snapshot.entries.len() as u64);
                assert_eq!(
                    snapshot.total_size_bytes,
                    snapshot.entries.iter().map(|entry| entry.size_bytes).sum::<u64>()
                );
                assert!(snapshot.total_size_bytes <= snapshot.capacity_bytes);
                assert!(snapshot.entries.windows(2).all(|entries| entries[0].sequence < entries[1].sequence));
                for entry in &snapshot.entries {
                    let tx = cbor::decode::<Tx>(&entry.original_bytes).unwrap();
                    assert_eq!(entry.transaction_id, tx.tx_id());
                    assert_eq!(entry.size_bytes as usize, entry.original_bytes.len());
                }
            }
        });
    }

    #[tokio::test]
    async fn queue_overflow_reports_gap_without_queue_capacity_and_can_resubscribe() {
        let seconds = Arc::new(AtomicU64::new(1));
        let clock = seconds.clone();
        let mempool = InMemoryMempool::new_with_admission_clock(MempoolConfig::default(), move || {
            SystemTime::UNIX_EPOCH + Duration::from_secs(clock.load(Ordering::Relaxed))
        });
        let (_, mut receiver) = mempool.subscribe().unwrap();
        mempool.insert(Tx::from_str("consumed").unwrap(), TxOrigin::Local);
        assert_eq!(receiver.recv().await.unwrap().generation(), 1);
        seconds.store(2, Ordering::Relaxed);
        for i in 0..=crate::inspection::MAX_MEMPOOL_QUEUED_EVENTS {
            mempool.insert(Tx::from_str(&i.to_string()).unwrap(), TxOrigin::Local);
        }
        assert_eq!(
            receiver.recv().await.unwrap_err(),
            PoolObserverError::Gap {
                expected_generation: 2,
                current_generation: crate::inspection::MAX_MEMPOOL_QUEUED_EVENTS as u64 + 2,
            }
        );
        seconds.store(3, Ordering::Relaxed);
        let (snapshot, mut fresh) = mempool.subscribe().unwrap();
        assert_eq!(snapshot.entries[0].admitted_at, SystemTime::UNIX_EPOCH + Duration::from_secs(1));
        assert!(
            snapshot.entries[1..]
                .iter()
                .all(|entry| { entry.admitted_at == SystemTime::UNIX_EPOCH + Duration::from_secs(2) })
        );
        mempool.insert(Tx::from_str("fresh").unwrap(), TxOrigin::Local);
        let event = fresh.recv().await.unwrap();
        assert_eq!(event.generation(), snapshot.generation + 1);
        assert!(matches!(event.as_ref(), PoolChange::Inserted { entry, .. }
            if entry.admitted_at == SystemTime::UNIX_EPOCH + Duration::from_secs(3)));
    }

    #[tokio::test]
    async fn payload_budget_also_invalidates_slow_receivers() {
        let limit = crate::inspection::MAX_MEMPOOL_QUEUED_BYTES;
        let mempool = InMemoryMempool::new(MempoolConfig::default().with_max_bytes(limit * 2));
        let (_, mut receiver) = mempool.subscribe().unwrap();
        assert!(matches!(
            mempool.insert(Tx("a".repeat(limit as usize)), TxOrigin::Local),
            TxInsertResult::Accepted { .. }
        ));
        assert_eq!(
            receiver.recv().await.unwrap_err(),
            PoolObserverError::Gap { expected_generation: 1, current_generation: 1 }
        );
        assert_eq!(mempool.snapshot().transaction_count, 1);
    }

    #[tokio::test]
    async fn subscriptions_are_bounded_reclaim_dropped_slots_and_do_not_retain_pool() {
        let mempool = InMemoryMempool::<Tx>::default();
        let mut receivers = Vec::new();
        for _ in 0..MAX_MEMPOOL_SUBSCRIBERS {
            receivers.push(mempool.subscribe().unwrap().1);
        }
        assert!(matches!(mempool.subscribe(), Err(PoolObserverError::SubscriberLimit)));
        receivers.pop();
        let (_, mut receiver) = mempool.subscribe().unwrap();
        drop(mempool);
        assert_eq!(receiver.recv().await.unwrap_err(), PoolObserverError::Stopped);
    }

    #[tokio::test]
    async fn forging_removals_also_advance_generation() {
        let mempool = InMemoryMempool::default();
        let tx = Tx::from_str("forged").unwrap();
        let (_, mut receiver) = mempool.subscribe().unwrap();
        mempool.insert(tx.clone(), TxOrigin::Local);
        receiver.recv().await.unwrap();
        mempool.acknowledge(&tx, |tx| [tx.tx_id()]);
        assert!(
            matches!(receiver.recv().await.unwrap().as_ref(), PoolChange::Removed { generation: 2, transaction_ids } if transaction_ids == &[tx.tx_id()])
        );
        mempool.insert(tx.clone(), TxOrigin::Local);
        receiver.recv().await.unwrap();
        assert_eq!(mempool.take(), vec![tx]);
        assert!(matches!(receiver.recv().await.unwrap().as_ref(), PoolChange::Removed { generation: 4, .. }));
        assert_eq!(mempool.snapshot().total_size_bytes, 0);
        assert!(mempool.take().is_empty());
        assert_eq!(mempool.snapshot().generation, 4);
    }

    // HELPERS
    #[derive(Debug, PartialEq, Eq, Clone, cbor::Encode, cbor::Decode)]
    struct Tx(#[n(0)] String);

    impl Deref for Tx {
        type Target = String;
        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    impl FromStr for Tx {
        type Err = ();
        fn from_str(s: &str) -> Result<Self, Self::Err> {
            Ok(Tx(s.to_string()))
        }
    }

    impl HasTransactionId for Tx {
        fn tx_id(&self) -> TransactionId {
            TransactionId::new(Hasher::<{ TRANSACTION_BODY * 8 }>::hash(&to_cbor(self)))
        }
    }
}
