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

use amaru_kernel::cbor::Encode;
use amaru_ouroboros_traits::mempool::{Mempool, TxId};
use amaru_ouroboros_traits::{MempoolSeqNo, TxOrigin, TxRejectReason};
use minicbor::CborLen;
use std::collections::{BTreeMap, BTreeSet};
use std::mem;
use std::pin::Pin;
use std::sync::Arc;

pub struct InMemoryMempool<Tx> {
    inner: parking_lot::RwLock<MempoolInner<Tx>>,
    config: MempoolConfig,
}

impl<Tx> Default for InMemoryMempool<Tx> {
    fn default() -> Self {
        InMemoryMempool {
            inner: parking_lot::RwLock::new(MempoolInner::default()),
            config: MempoolConfig::default(),
        }
    }
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

impl<Tx: Encode<()> + CborLen<()>> MempoolInner<Tx> {
    fn insert(
        &mut self,
        config: &MempoolConfig,
        tx: Tx,
        tx_origin: TxOrigin,
    ) -> Result<(TxId, MempoolSeqNo), TxRejectReason> {
        if let Some(max_txs) = config.max_txs
            && self.entries_by_id.len() >= max_txs
        {
            return Err(TxRejectReason::MempoolFull);
        }

        let tx_id = TxId::from(&tx);
        if self.entries_by_id.contains_key(&tx_id) {
            return Err(TxRejectReason::Duplicate);
        }

        let seq_no = MempoolSeqNo(self.next_seq);
        self.next_seq += 1;

        let tx_size = minicbor::len(&tx) as u32;
        let entry = MempoolEntry {
            seq_no,
            tx_id: tx_id.clone(),
            tx: Arc::new(tx),
            tx_size,
            origin: tx_origin,
        };

        self.entries_by_id.insert(tx_id.clone(), entry);
        self.entries_by_seq.insert(seq_no, tx_id.clone());
        Ok((tx_id, seq_no))
    }

    fn get_tx(&self, tx_id: &TxId) -> Option<Arc<Tx>> {
        self.entries_by_id.get(tx_id).map(|entry| entry.tx.clone())
    }

    fn tx_ids_since(&self, from_seq: MempoolSeqNo, limit: u16) -> Vec<(TxId, u32, MempoolSeqNo)> {
        let mut result: Vec<(TxId, u32, MempoolSeqNo)> = self
            .entries_by_seq
            .range(from_seq..)
            .take(limit as usize)
            .map(|(seq, tx_id)| {
                if let Some(entry) = self.entries_by_id.get(tx_id) {
                    let tx_size = entry.tx_size;
                    (tx_id.clone(), tx_size, *seq)
                } else {
                    panic!(
                        "Inconsistent mempool state: entry missing for tx_id {:?}",
                        tx_id
                    );
                }
            })
            .collect();
        result.sort_by_key(|(_, _, seq_no)| seq_no.clone());
        result
    }

    fn get_txs_for_ids(&self, ids: &[TxId]) -> Vec<Arc<Tx>> {
        // Make sure that the result are sorted by seq_no
        let mut result: Vec<(&TxId, &MempoolEntry<Tx>)> = self.entries_by_id
            .iter()
            .filter(|(key, _)| ids.contains(*key)).collect();
        result.sort_by_key(|(_, entry)| entry.seq_no);
        result.into_iter().map(|(_, entry)| entry.tx.clone()).collect()
    }
}

#[derive(Debug, Clone)]
pub struct MempoolEntry<Tx> {
    seq_no: MempoolSeqNo,
    tx_id: TxId,
    tx: Arc<Tx>,
    tx_size: u32,
    origin: TxOrigin,
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

impl<Tx: Send + Sync + 'static + Encode<()> + CborLen<()>> Mempool<Tx> for InMemoryMempool<Tx> {
    fn insert(&self, tx: Tx, tx_origin: TxOrigin) -> Result<(TxId, MempoolSeqNo), TxRejectReason> {
        self.inner.write().insert(&self.config, tx, tx_origin)
    }

    fn take(&self) -> Vec<Tx> {
        let mut inner = self.inner.write();
        let entries = mem::take(&mut inner.entries_by_id);
        let _ = mem::take(&mut inner.entries_by_seq);
        entries
            .into_values()
            .filter_map(|entry| Arc::into_inner(entry.tx))
            .collect()
    }

    fn acknowledge<TxKey: Ord, I>(&self, tx: &Tx, keys: fn(&Tx) -> I)
    where
        I: IntoIterator<Item=TxKey>,
        Self: Sized,
    {
        let keys_to_remove = BTreeSet::from_iter(keys(tx).into_iter());
        let mut inner = self.inner.write();
        inner.entries_by_id.retain(|_, entry| {
            !keys(&entry.tx)
                .into_iter()
                .any(|k| keys_to_remove.contains(&k))
        });
    }

    fn get_tx(&self, tx_id: &TxId) -> Option<Arc<Tx>> {
        self.inner.read().get_tx(tx_id)
    }

    fn tx_ids_since(&self, from_seq: MempoolSeqNo, limit: u16) -> Vec<(TxId, u32, MempoolSeqNo)> {
        self.inner.read().tx_ids_since(from_seq, limit)
    }

    fn wait_for_at_least(&self, _required: u16) -> Pin<Box<dyn Future<Output=bool> + Send + '_>> {
        Box::pin(async move {
            loop {
                // TODO: make sure that transactions are valid before returning
                // So we can make sure to send enough valid transactions upstream
                return true;
                // when a significant validation is implemented we will yield here
                // tokio::task::yield_now().await;
            }
        })
    }

    fn get_txs_for_ids(&self, ids: &[TxId]) -> Vec<Arc<Tx>> {
        self.inner.read().get_txs_for_ids(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use amaru_kernel::peer::Peer;
    use assertables::assert_some_eq_x;
    use minicbor::Decode;
    use std::str::FromStr;

    #[test]
    fn insert_a_transaction() {
        let mempool = InMemoryMempool::new(MempoolConfig::default().with_max_txs(5));
        let tx = Tx::from_str("tx1").unwrap();
        let (tx_id, seq_nb) = mempool
            .insert(tx.clone(), TxOrigin::Remote(Peer::new("peer-1")))
            .unwrap();

        assert_some_eq_x!(mempool.get_tx(&tx_id), Arc::new(tx.clone()));
        assert_eq!(
            mempool.get_txs_for_ids(&[tx_id.clone()]),
            vec![Arc::new(tx.clone())]
        );
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
