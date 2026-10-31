// Copyright 2025 PRAGMA
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

use std::{collections::BTreeMap, sync::Arc};

use amaru_kernel::{HasTransactionId, TransactionId, cbor, to_cbor};
use amaru_ouroboros_traits::{Mempool, MempoolRead, MempoolSeqNo, TxInsertResult, TxOrigin, TxRejectReason};
use parking_lot::RwLockReadGuard;

use super::MempoolConfig;

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
    entries_by_id: BTreeMap<TransactionId, MempoolSeqNo>,
    entries_by_seq: BTreeMap<MempoolSeqNo, MempoolEntry<Tx>>,
}

impl<Tx> Default for InMemoryMempoolInner<Tx> {
    fn default() -> Self {
        InMemoryMempoolInner {
            next_seq: 1,
            current_bytes: 0,
            entries_by_id: Default::default(),
            entries_by_seq: Default::default(),
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

        let entry = MempoolEntry { seq_no, tx_id, tx, tx_size, origin: tx_origin };

        self.entries_by_id.insert(tx_id, seq_no);
        self.entries_by_seq.insert(seq_no, entry);
        self.current_bytes = self.current_bytes.saturating_add(tx_size as u64);
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
        for tx_id in ids {
            if let Some(seq_no) = self.entries_by_id.remove(tx_id)
                && let Some(entry) = self.entries_by_seq.remove(&seq_no)
            {
                self.current_bytes = self.current_bytes.saturating_sub(entry.tx_size as u64);
            }
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
}

#[cfg(test)]
mod tests {
    use std::{ops::Deref, slice, str::FromStr};

    use amaru_kernel::{Hasher, Peer, cbor, cbor as minicbor, size::TRANSACTION_BODY};
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
