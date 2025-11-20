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

use crate::mempool::{Mempool, TxId};
use amaru_kernel::cbor::Encode;
use amaru_kernel::peer::Peer;
use std::collections::{BTreeMap, BTreeSet};
use std::mem;
use std::ops::Deref;
use std::sync::Arc;

pub struct InMemoryMempool<Tx> {
    inner: parking_lot::RwLock<MempoolInner<Tx>>,
    config: MempoolConfig,
}

impl<Tx> InMemoryMempool<Tx> {
    pub fn new(config: MempoolConfig) -> Self {
        InMemoryMempool {
            inner: parking_lot::RwLock::new(MempoolInner::default()),
            config,
        }
    }
}

#[derive(Debug)]
struct MempoolInner<Tx> {
    next_seq: u64,
    entries_by_id: BTreeMap<TxId, MempoolEntry<Tx>>,
    entries_by_seq: BTreeMap<MempoolSeqNo, TxId>,
}

impl<Tx> Default for MempoolInner<Tx> {
    fn default() -> Self {
        MempoolInner {
            next_seq: 0,
            entries_by_id: Default::default(),
            entries_by_seq: Default::default(),
        }
    }
}

impl<Tx: Encode<()>> MempoolInner<Tx> {
    fn insert(
        &mut self,
        config: &MempoolConfig,
        tx: Tx,
        tx_origin: TxOrigin,
    ) -> Result<(TxId, MempoolSeqNo), TxRejectReason> {
        if let Some(max_txs) = config.max_txs && self.entries_by_id.len() >= max_txs {
            return Err(TxRejectReason::MempoolFull);
        }

        let tx_id = TxId::from(&tx);
        if self.entries_by_id.contains_key(&tx_id) {
            return Err(TxRejectReason::Duplicate);
        }

        let seq_no = MempoolSeqNo(self.next_seq);
        self.next_seq += 1;

        let entry = MempoolEntry {
            seq_no,
            tx_id: tx_id.clone(),
            tx: Arc::new(tx),
            origin: tx_origin,
        };

        self.entries_by_id.insert(tx_id.clone(), entry);
        self.entries_by_seq.insert(seq_no, tx_id.clone());
        Ok((tx_id, seq_no))
    }

    fn get_tx(&self, tx_id: &TxId) -> Option<Arc<Tx>> {
        self.entries_by_id.get(tx_id).map(|entry| entry.tx.clone())
    }

    fn tx_ids_since(&self, from_seq: MempoolSeqNo, limit: usize) -> Vec<(TxId, MempoolSeqNo)> {
        self.entries_by_seq
            .range(from_seq..)
            .take(limit)
            .map(|(seq, tx_id)| (tx_id.clone(), *seq))
            .collect()
    }

    fn get_txs_for_ids(&self, ids: &[TxId]) -> Vec<Arc<Tx>> {
        self.entries_by_id
            .iter().filter(|(key, _)| ids.contains(*key)).map(|(_, entry)| entry.tx.clone())
            .collect()
    }
}


#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MempoolSeqNo(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TxRejectReason {
    MempoolFull,
    Duplicate,
    Invalid,
}

#[derive(Debug, Clone)]
pub struct MempoolEntry<Tx> {
    seq_no: MempoolSeqNo,
    tx_id: TxId,
    tx: Arc<Tx>,
    origin: TxOrigin,
}

#[derive(Debug, Clone)]
pub enum TxOrigin {
    Local,
    Remote(Peer),
}

#[derive(Debug, Clone, Default)]
pub struct MempoolConfig {
    max_txs: Option<usize>,
}

impl MempoolConfig {
    pub fn with_max_txs(mut self, max: usize) -> Self {
        self.max_txs = Some(max);
        self
    }
}


impl<Tx: Deref + Send + Sync + 'static + Encode<()>> Mempool<Tx> for InMemoryMempool<Tx>
{
    fn insert(&self, tx: Tx, tx_origin: TxOrigin) -> Result<(TxId, MempoolSeqNo), TxRejectReason> {
        self.inner.write().insert(&self.config, tx, tx_origin)
    }

    fn take(&self) -> Vec<Tx> {
        let mut inner = self.inner.write();
        let entries = mem::take(&mut inner.entries_by_id);
        let _ = mem::take(&mut inner.entries_by_seq);
        entries.into_values().filter_map(|entry| Arc::into_inner(entry.tx)).collect()
    }

    fn acknowledge<TxKey: Ord, I>(&self, tx: &Tx, keys: fn(&Tx) -> I)
    where
        I: IntoIterator<Item=TxKey>,
    {
        let keys_to_remove = BTreeSet::from_iter(keys(tx).into_iter());
        let mut inner = self.inner.write();
        inner.entries_by_id.retain(|_, entry| !keys(&entry.tx).into_iter().any(|k| keys_to_remove.contains(&k)));
    }

    fn get_tx(&self, tx_id: &TxId) -> Option<Arc<Tx>> {
        self.inner.read().get_tx(tx_id)
    }

    fn tx_ids_since(&self, from_seq: MempoolSeqNo, limit: usize) -> Vec<(TxId, MempoolSeqNo)> {
        self.inner.read().tx_ids_since(from_seq, limit)
    }

    fn get_txs_for_ids(&self, ids: &[TxId]) -> Vec<Arc<Tx>> {
        self.inner.read().get_txs_for_ids(ids)
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;
    use assertables::{assert_some_eq_x};
    use minicbor::{Decode};
    use amaru_kernel::peer::Peer;
    use super::*;

    #[test]
    fn insert_a_transaction() {
        let mempool = InMemoryMempool::new(MempoolConfig::default().with_max_txs(5));
        let tx = Tx::from_str("tx1").unwrap();
        let (tx_id, seq_nb) = mempool.insert(tx.clone(), TxOrigin::Remote(Peer::new("peer-1"))).unwrap();

        assert_some_eq_x!(mempool.get_tx(&tx_id), Arc::new(tx.clone()));
        assert_eq!(mempool.get_txs_for_ids(&[tx_id.clone()]), vec![Arc::new(tx.clone())]);
        assert_eq!(mempool.tx_ids_since(seq_nb, 100), vec![(tx_id, seq_nb)]);
    }


    // HELPERS
    #[derive(Debug, PartialEq, Eq, Clone, Encode, Decode)]
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
