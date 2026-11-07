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

use std::{collections::BTreeMap, sync::Arc, time::SystemTime};

use amaru_kernel::{HasTransactionId, Point, TransactionId, cbor, to_cbor};
use amaru_ouroboros_traits::{Mempool, MempoolRead, MempoolSeqNo, TxInsertResult, TxOrigin, TxRejectReason};
use parking_lot::RwLockReadGuard;
use tokio::sync::broadcast;

use super::MempoolConfig;
use crate::inspection::{MAX_MEMPOOL_QUEUED_EVENTS, MempoolSnapshotEntry, PoolChange, PoolReceiver, PoolSnapshot};

/// A temporary in-memory mempool implementation to support the transaction submission protocol.
///
/// It stores transactions in memory, indexed by their TransactionId and by a sequence number assigned
/// at insertion time.
///
#[derive(Clone)]
pub struct InMemoryMempool<Tx> {
    config: MempoolConfig,
    inner: Arc<parking_lot::RwLock<InMemoryMempoolInner<Tx>>>,
}

impl<Tx: 'static> Default for InMemoryMempool<Tx> {
    fn default() -> Self {
        Self::new(MempoolConfig::default())
    }
}

impl<Tx> InMemoryMempool<Tx> {
    /// Create an empty mempool with the supplied capacity configuration.
    pub fn new(config: MempoolConfig) -> Self {
        InMemoryMempool { config, inner: Arc::new(parking_lot::RwLock::new(InMemoryMempoolInner::default())) }
    }
}

#[derive(Debug)]
struct InMemoryMempoolInner<Tx> {
    next_seq: u64,
    current_bytes: u64,
    tip: Point,
    entries_by_id: BTreeMap<TransactionId, MempoolSeqNo>,
    entries_by_seq: BTreeMap<MempoolSeqNo, MempoolEntry<Tx>>,
    changes: broadcast::Sender<Arc<PoolChange>>,
}

impl<Tx> Default for InMemoryMempoolInner<Tx> {
    fn default() -> Self {
        InMemoryMempoolInner {
            next_seq: 1,
            current_bytes: 0,
            tip: Point::Origin,
            entries_by_id: Default::default(),
            entries_by_seq: Default::default(),
            changes: broadcast::channel(MAX_MEMPOOL_QUEUED_EVENTS).0,
        }
    }
}

impl<Tx: HasTransactionId + cbor::Encode<()>> InMemoryMempoolInner<Tx> {
    /// Inserts a new transaction into the mempool.
    /// The transaction id is a hash of the transaction body.
    fn insert(
        &mut self,
        config: &MempoolConfig,
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

        let seq_no = MempoolSeqNo(self.next_seq);
        self.next_seq += 1;

        let entry = MempoolEntry { seq_no, tx_id, tx, tx_size, origin: tx_origin, admitted_at: SystemTime::now() };

        self.entries_by_id.insert(tx_id, seq_no);
        self.entries_by_seq.insert(seq_no, entry);
        self.current_bytes = self.current_bytes.saturating_add(tx_size as u64);
        if self.changes.receiver_count() > 0 {
            let _ = self.changes.send(Arc::new(PoolChange::Inserted(self.entries_by_seq[&seq_no].snapshot())));
        }
        Ok((tx_id, seq_no))
    }

    /// Return transaction ids from `from_seq` (inclusive), up to `limit` entries.
    ///
    /// The sequence-indexed map makes the result insertion-ordered. Removed transactions are
    /// skipped, while sequence numbers remain monotonic and are never reused.
    fn slice(&self, from_seq: MempoolSeqNo, limit: u16) -> Vec<(TransactionId, u32, MempoolSeqNo)> {
        self.entries_by_seq
            .range(from_seq..)
            .take(limit as usize)
            .map(|(seq, entry)| (entry.tx_id, entry.tx_size, *seq))
            .collect()
    }

    /// Remove transactions by id and release their accounted CBOR size.
    fn remove(&mut self, ids: &[TransactionId]) {
        let mut removed = Vec::new();
        for tx_id in ids {
            if let Some(seq_no) = self.entries_by_id.remove(tx_id)
                && let Some(entry) = self.entries_by_seq.remove(&seq_no)
            {
                self.current_bytes = self.current_bytes.saturating_sub(entry.tx_size as u64);
                removed.push(*tx_id);
            }
        }
        if !removed.is_empty() {
            let _ = self.changes.send(Arc::new(PoolChange::Removed(removed)));
        }
    }
}

struct InMemoryMempoolRead<'a, Tx> {
    inner: RwLockReadGuard<'a, InMemoryMempoolInner<Tx>>,
}

impl<Tx: Send + Sync + 'static> MempoolRead<Tx> for InMemoryMempoolRead<'_, Tx> {
    fn get(&self, ids: &[TransactionId]) -> Vec<&Tx> {
        let mut entries: Vec<&MempoolEntry<Tx>> = ids
            .iter()
            .filter_map(|tx_id| {
                self.inner.entries_by_id.get(tx_id).and_then(|seq_no| self.inner.entries_by_seq.get(seq_no))
            })
            .collect();
        entries.sort_by_key(|entry| entry.seq_no);
        entries.into_iter().map(|entry| &entry.tx).collect()
    }

    /// Iterate over the sequence-indexed map, so results are insertion-ordered without a second
    /// lookup or cloning the transactions.
    fn iter(&self) -> Box<dyn Iterator<Item = &Tx> + '_> {
        Box::new(self.inner.entries_by_seq.values().map(|entry| &entry.tx))
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
    fn snapshot_inner(&self, inner: &InMemoryMempoolInner<Tx>) -> PoolSnapshot {
        PoolSnapshot {
            entries: inner.entries_by_seq.values().map(MempoolEntry::snapshot).collect(),
            transaction_count: inner.entries_by_id.len() as u64,
            total_size_bytes: inner.current_bytes,
            capacity_bytes: self.config.max_bytes,
        }
    }

    /// Capture membership, insertion order, and counters in one read.
    pub fn snapshot(&self) -> PoolSnapshot {
        self.snapshot_inner(&self.inner.read())
    }

    /// Atomically capture an initial snapshot and register for all later membership changes.
    pub fn subscribe(&self) -> (PoolSnapshot, PoolReceiver) {
        let inner = self.inner.read();
        (self.snapshot_inner(&inner), inner.changes.subscribe())
    }
}

impl<Tx: Send + Sync + 'static + HasTransactionId + cbor::Encode<()>> Mempool<Tx> for InMemoryMempool<Tx> {
    fn insert(&self, tx: Tx, tx_origin: TxOrigin) -> TxInsertResult {
        let tx_id = tx.tx_id();
        let mut inner = self.inner.write();
        let res = inner.insert(&self.config, tx, tx_origin);
        match res {
            Ok((tx_id, seq_no)) => TxInsertResult::accepted(tx_id, seq_no),
            Err(reason) => TxInsertResult::rejected(tx_id, reason),
        }
    }

    fn read(&self) -> Box<dyn MempoolRead<Tx> + '_> {
        Box::new(InMemoryMempoolRead { inner: self.inner.read() })
    }

    fn read_at(&self, tip: Point) -> Option<Box<dyn MempoolRead<Tx> + '_>> {
        let inner = self.inner.read();
        (inner.tip == tip).then(|| Box::new(InMemoryMempoolRead { inner }) as Box<dyn MempoolRead<Tx>>)
    }

    fn slice(&self, from_seq: MempoolSeqNo, limit: u16) -> Vec<(TransactionId, u32, MempoolSeqNo)> {
        self.inner.read().slice(from_seq, limit)
    }

    fn remove(&self, ids: &[TransactionId]) {
        self.inner.write().remove(ids)
    }

    fn last(&self) -> MempoolSeqNo {
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

    fn set_tip(&self, tip: Point) {
        self.inner.write().tip = tip;
    }
}

#[cfg(test)]
mod tests {
    use std::{ops::Deref, slice, str::FromStr};

    use amaru_kernel::{Hash, Hasher, Peer, Point, cbor, cbor as minicbor, size::TRANSACTION_BODY};
    use amaru_ouroboros_traits::TxRejectReason;

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

        let read = mempool.read();
        assert_eq!(read.get(slice::from_ref(&tx_id)), vec![&tx]);
        assert_eq!(mempool.slice(seq_nb, 100), vec![(tx_id, 5, seq_nb)]);
        assert_eq!(mempool.last(), seq_nb);
        Ok(())
    }

    #[test]
    fn iterates_transactions_in_insertion_order_without_cloning() {
        let mempool = InMemoryMempool::new(MempoolConfig::default());
        let first = Tx::from_str("first").unwrap();
        let second = Tx::from_str("second").unwrap();
        let first_id = first.tx_id();
        let second_id = second.tx_id();

        assert!(matches!(mempool.insert(first, TxOrigin::Local), TxInsertResult::Accepted { .. }));
        assert!(matches!(mempool.insert(second, TxOrigin::Local), TxInsertResult::Accepted { .. }));

        let ids = mempool.read().iter().map(|tx| tx.tx_id()).collect::<Vec<_>>();
        assert_eq!(ids, vec![first_id, second_id]);
    }

    #[test]
    fn read_at_rejects_a_stale_tip_without_exposing_a_view() {
        let mempool = InMemoryMempool::new(MempoolConfig::default());
        let tip = Point::Specific(1.into(), Hash::new([1u8; 32]), 1.into());
        let tx = Tx::from_str("tx").unwrap();
        mempool.insert(tx.clone(), TxOrigin::Local);
        mempool.set_tip(tip);

        assert!(mempool.read_at(Point::Origin).is_none());
        assert_eq!(mempool.read_at(tip).unwrap().iter().collect::<Vec<_>>(), vec![&tx]);
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
    fn remove_frees_bytes_capacity() {
        let first = Tx::from_str("a").unwrap();
        let max_bytes = to_cbor(&first).len() as u64;
        let mempool = InMemoryMempool::new(MempoolConfig::default().with_max_bytes(max_bytes));

        let TxInsertResult::Accepted { tx_id, .. } = mempool.insert(first, TxOrigin::Local) else {
            panic!("first insert should succeed");
        };

        mempool.remove(&[tx_id]);

        let second = Tx::from_str("b").unwrap();
        assert!(matches!(mempool.insert(second, TxOrigin::Local), TxInsertResult::Accepted { .. }));
    }

    #[tokio::test]
    async fn snapshots_and_events_follow_membership_and_report_lag() {
        let mempool = InMemoryMempool::default();
        let first = Tx::from_str("first").unwrap();
        let second = Tx::from_str("second").unwrap();
        let (empty, mut changes) = mempool.subscribe();
        assert!(empty.entries.is_empty());
        mempool.insert(first.clone(), TxOrigin::Local);
        let snapshot = mempool.snapshot();
        assert!(matches!(changes.recv().await.unwrap().as_ref(), PoolChange::Inserted(entry)
            if entry == &snapshot.entries[0] && entry.original_bytes == to_cbor(&first)));
        mempool.insert(first.clone(), TxOrigin::Local);
        assert_eq!(mempool.snapshot(), snapshot);
        mempool.insert(second.clone(), TxOrigin::Local);
        assert!(
            matches!(changes.recv().await.unwrap().as_ref(), PoolChange::Inserted(entry) if entry.transaction_id == second.tx_id())
        );
        mempool.remove(&[first.tx_id()]);
        assert!(matches!(changes.recv().await.unwrap().as_ref(), PoolChange::Removed(transaction_ids)
            if transaction_ids == &[first.tx_id()]));
        assert_eq!(mempool.snapshot().entries[0].transaction_id, second.tx_id());
        assert_eq!(mempool.snapshot().total_size_bytes, to_cbor(&second).len() as u64);
        mempool.remove(&[second.tx_id()]);
        assert!(matches!(changes.recv().await.unwrap().as_ref(), PoolChange::Removed(_)));
        mempool.remove(&[first.tx_id()]);
        mempool.remove(&[]);
        assert_eq!(mempool.snapshot().total_size_bytes, 0);
        for i in 0..=MAX_MEMPOOL_QUEUED_EVENTS {
            mempool.insert(Tx(i.to_string()), TxOrigin::Local);
        }
        assert!(matches!(changes.recv().await, Err(broadcast::error::RecvError::Lagged(1))));
        let (snapshot, mut fresh) = mempool.subscribe();
        assert_eq!(snapshot.transaction_count, MAX_MEMPOOL_QUEUED_EVENTS as u64 + 1);
        mempool.insert(first.clone(), TxOrigin::Local);
        assert!(
            matches!(fresh.recv().await.unwrap().as_ref(), PoolChange::Inserted(entry) if entry.transaction_id == first.tx_id())
        );
        drop(mempool);
        assert!(matches!(fresh.recv().await, Err(broadcast::error::RecvError::Closed)));
    }

    #[test]
    fn rejected_insertions_and_empty_removals_do_not_publish_changes() {
        let first = Tx::from_str("first").unwrap();
        let mempool = InMemoryMempool::new(MempoolConfig::default().with_max_bytes(to_cbor(&first).len() as u64));
        mempool.insert(first.clone(), TxOrigin::Local);
        let (snapshot, mut receiver) = mempool.subscribe();
        mempool.insert(first.clone(), TxOrigin::Remote(Peer::for_test(3005)));
        mempool.insert(Tx::from_str("second").unwrap(), TxOrigin::Local);
        mempool.remove(&[Tx::from_str("missing").unwrap().tx_id()]);
        assert_eq!(mempool.snapshot(), snapshot);
        assert!(matches!(receiver.try_recv(), Err(broadcast::error::TryRecvError::Empty)));
        mempool.remove(&[first.tx_id(), first.tx_id()]);
        assert!(matches!(receiver.try_recv().unwrap().as_ref(), PoolChange::Removed(transaction_ids)
            if transaction_ids == &[first.tx_id()]));
        assert_eq!(mempool.snapshot().total_size_bytes, 0);
    }

    #[test]
    fn subscription_has_no_gap_or_overlap_with_its_snapshot() {
        for _ in 0..20 {
            let mempool = InMemoryMempool::default();
            let transactions: Vec<_> = (0..16).map(|i| Tx(i.to_string())).collect();
            let expected: Vec<_> = transactions.iter().map(HasTransactionId::tx_id).collect();
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let writer = mempool.clone();
            let writer_barrier = barrier.clone();
            let thread = std::thread::spawn(move || {
                writer_barrier.wait();
                for tx in transactions {
                    writer.insert(tx, TxOrigin::Local);
                }
            });
            barrier.wait();
            let (snapshot, mut receiver) = mempool.subscribe();
            thread.join().unwrap();
            let mut observed: Vec<_> = snapshot.entries.iter().map(|entry| entry.transaction_id).collect();
            while let Ok(event) = receiver.try_recv() {
                let PoolChange::Inserted(entry) = event.as_ref() else { panic!("unexpected removal") };
                observed.push(entry.transaction_id);
            }
            assert_eq!(observed, expected);
        }
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
