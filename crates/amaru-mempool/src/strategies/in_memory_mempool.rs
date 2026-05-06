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

use std::{
    collections::{BTreeMap, BTreeSet},
    mem,
    sync::Arc,
};

use amaru_kernel::{cbor, to_cbor};
use amaru_ouroboros_traits::{
    MempoolSeqNo, TxId, TxInsertResult, TxOrigin, TxRejectReason, TxSubmissionMempool, mempool::Mempool,
};
/// A temporary in-memory mempool implementation to support the transaction submission protocol.
///
/// It stores transactions in memory, indexed by their TxId and by a sequence number assigned
/// at insertion time.
///
#[derive(Clone)]
pub struct InMemoryMempool<Tx> {
    config: MempoolConfig,
    inner: Arc<parking_lot::RwLock<MempoolInner<Tx>>>,
}

impl<Tx: 'static> Default for InMemoryMempool<Tx> {
    fn default() -> Self {
        Self::new(MempoolConfig::default())
    }
}

impl<Tx> InMemoryMempool<Tx> {
    pub fn new(config: MempoolConfig) -> Self {
        InMemoryMempool { config, inner: Arc::new(parking_lot::RwLock::new(MempoolInner::default())) }
    }
}

#[derive(Debug)]
struct MempoolInner<Tx> {
    next_seq: u64,
    current_bytes: u64,
    entries_by_id: BTreeMap<TxId, MempoolEntry<Tx>>,
    entries_by_seq: BTreeMap<MempoolSeqNo, TxId>,
}

impl<Tx> Default for MempoolInner<Tx> {
    fn default() -> Self {
        MempoolInner {
            next_seq: 1,
            current_bytes: 0,
            entries_by_id: Default::default(),
            entries_by_seq: Default::default(),
        }
    }
}

impl<Tx: cbor::Encode<()> + Clone> MempoolInner<Tx> {
    /// Inserts a new transaction into the mempool.
    /// The transaction id is a hash of the transaction body.
    fn insert(
        &mut self,
        config: &MempoolConfig,
        tx: Tx,
        tx_origin: TxOrigin,
    ) -> Result<(TxId, MempoolSeqNo), TxRejectReason> {
        let tx_size = to_cbor(&tx).len() as u32;
        let tx_id = TxId::from(&tx);

        if self.entries_by_id.contains_key(&tx_id) {
            return Err(TxRejectReason::Duplicate);
        }

        if self.current_bytes.saturating_add(tx_size as u64) > config.max_bytes {
            return Err(TxRejectReason::MempoolFull);
        }

        let seq_no = MempoolSeqNo(self.next_seq);
        self.next_seq += 1;

        let entry = MempoolEntry { seq_no, tx_id, tx, tx_size, origin: tx_origin };

        self.entries_by_id.insert(tx_id, entry);
        self.entries_by_seq.insert(seq_no, tx_id);
        self.current_bytes = self.current_bytes.saturating_add(tx_size as u64);
        Ok((tx_id, seq_no))
    }

    /// Retrieves a transaction by its id.
    fn get_tx(&self, tx_id: &TxId) -> Option<Tx> {
        self.entries_by_id.get(tx_id).map(|entry| entry.tx.clone())
    }

    /// Retrieves all the transaction ids since a given sequence number, up to a limit.
    fn tx_ids_since(&self, from_seq: MempoolSeqNo, limit: u16) -> Vec<(TxId, u32, MempoolSeqNo)> {
        let mut result: Vec<(TxId, u32, MempoolSeqNo)> = self
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
    fn get_txs_for_ids(&self, ids: &[TxId]) -> Vec<Tx> {
        // Make sure that the result are sorted by seq_no
        let mut result: Vec<(&TxId, &MempoolEntry<Tx>)> =
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

    fn remove_txs(&mut self, ids: &[TxId]) {
        for tx_id in ids {
            if let Some(entry) = self.entries_by_id.remove(tx_id) {
                self.entries_by_seq.remove(&entry.seq_no);
                self.current_bytes = self.current_bytes.saturating_sub(entry.tx_size as u64);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct MempoolEntry<Tx> {
    seq_no: MempoolSeqNo,
    tx_id: TxId,
    tx: Tx,
    tx_size: u32,
    origin: TxOrigin,
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

impl<Tx: Send + Sync + 'static + cbor::Encode<()> + Clone> TxSubmissionMempool<Tx> for InMemoryMempool<Tx> {
    fn insert(&self, tx: Tx, tx_origin: TxOrigin) -> Result<TxInsertResult, amaru_ouroboros_traits::MempoolError> {
        let tx_id = TxId::from(&tx);
        let mut inner = self.inner.write();
        let res = inner.insert(&self.config, tx, tx_origin);
        Ok(match res {
            Ok((tx_id, seq_no)) => TxInsertResult::accepted(tx_id, seq_no),
            Err(reason) => TxInsertResult::rejected(tx_id, reason),
        })
    }

    fn get_tx(&self, tx_id: &TxId) -> Option<Tx> {
        self.inner.read().get_tx(tx_id)
    }

    fn tx_ids_since(&self, from_seq: MempoolSeqNo, limit: u16) -> Vec<(TxId, u32, MempoolSeqNo)> {
        self.inner.read().tx_ids_since(from_seq, limit)
    }

    fn get_txs_for_ids(&self, ids: &[TxId]) -> Vec<Tx> {
        self.inner.read().get_txs_for_ids(ids)
    }

    fn mempool_txs(&self) -> Vec<Tx> {
        self.inner.read().mempool_txs()
    }

    fn remove_txs(&self, ids: &[TxId]) -> Result<(), amaru_ouroboros_traits::MempoolError> {
        self.inner.write().remove_txs(ids);
        Ok(())
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

impl<Tx: Send + Sync + 'static + cbor::Encode<()> + Clone> Mempool<Tx> for InMemoryMempool<Tx> {
    fn take(&self) -> Vec<Tx> {
        let mut inner = self.inner.write();
        let entries = mem::take(&mut inner.entries_by_id);
        let _ = mem::take(&mut inner.entries_by_seq);
        inner.current_bytes = 0;
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
        let mut bytes_to_subtract: u64 = 0;
        for entry in inner.entries_by_id.values() {
            if keys(&entry.tx).into_iter().any(|k| keys_to_remove.contains(&k)) {
                seq_nos_to_remove.push(entry.seq_no);
                bytes_to_subtract = bytes_to_subtract.saturating_add(entry.tx_size as u64);
            }
        }
        inner.entries_by_id.retain(|_, entry| !keys(&entry.tx).into_iter().any(|k| keys_to_remove.contains(&k)));
        for seq_no in seq_nos_to_remove {
            inner.entries_by_seq.remove(&seq_no);
        }
        inner.current_bytes = inner.current_bytes.saturating_sub(bytes_to_subtract);
    }
}

#[cfg(test)]
mod tests {
    use std::{ops::Deref, slice, str::FromStr};

    use amaru_kernel::{Peer, cbor, cbor as minicbor};
    use amaru_ouroboros_traits::TxRejectReason;
    use assertables::assert_some_eq_x;

    use super::*;

    #[tokio::test]
    async fn insert_a_transaction() -> anyhow::Result<()> {
        let mempool = InMemoryMempool::new(MempoolConfig::default());
        let tx = Tx::from_str("tx1").unwrap();
        let TxInsertResult::Accepted { tx_id, seq_no: seq_nb } =
            mempool.insert(tx.clone(), TxOrigin::Remote(Peer::new("upstream"))).unwrap()
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

        assert!(matches!(mempool.insert(first, TxOrigin::Local).unwrap(), TxInsertResult::Accepted { .. }));

        let TxInsertResult::Rejected { reason, .. } =
            mempool.insert(Tx::from_str("b").unwrap(), TxOrigin::Local).unwrap()
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

        assert!(matches!(mempool.insert(first.clone(), TxOrigin::Local).unwrap(), TxInsertResult::Accepted { .. }));

        let TxInsertResult::Rejected { reason, .. } = mempool.insert(first, TxOrigin::Local).unwrap() else {
            panic!("transaction should be rejected");
        };
        assert!(matches!(reason, TxRejectReason::Duplicate), "unexpected reason: {reason:?}");
    }

    #[test]
    fn remove_txs_frees_bytes_capacity() {
        let first = Tx::from_str("a").unwrap();
        let max_bytes = to_cbor(&first).len() as u64;
        let mempool = InMemoryMempool::new(MempoolConfig::default().with_max_bytes(max_bytes));

        let TxInsertResult::Accepted { tx_id, .. } = mempool.insert(first, TxOrigin::Local).unwrap() else {
            panic!("first insert should succeed");
        };

        mempool.remove_txs(&[tx_id]).unwrap();

        let second = Tx::from_str("b").unwrap();
        assert!(matches!(mempool.insert(second, TxOrigin::Local).unwrap(), TxInsertResult::Accepted { .. }));
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
}
