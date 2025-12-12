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
use amaru_kernel::to_cbor;
use amaru_ouroboros_traits::mempool::Mempool;
use amaru_ouroboros_traits::{
    CanValidateTransactions, MempoolSeqNo, TransactionValidationError, TxId, TxOrigin,
    TxRejectReason, TxSubmissionMempool,
};
use std::collections::{BTreeMap, BTreeSet};
use std::mem;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::Notify;

/// A temporary in-memory mempool implementation to support the transaction submission protocol.
///
/// It stores transactions in memory, indexed by their TxId and by a sequence number assigned
/// at insertion time.
///
/// The validation of the transactions are delegated to a `CanValidateTransactions` implementation.
///
pub struct InMemoryMempool<Tx> {
    config: MempoolConfig,
    inner: parking_lot::RwLock<MempoolInner<Tx>>,
    notify: Notify,
    tx_validator: Arc<dyn CanValidateTransactions<Tx>>,
}

impl<Tx> Default for InMemoryMempool<Tx> {
    fn default() -> Self {
        InMemoryMempool {
            config: MempoolConfig::default(),
            inner: parking_lot::RwLock::new(MempoolInner::default()),
            notify: Notify::new(),
            tx_validator: Arc::new(DefaultCanValidateTransactions),
        }
    }
}

impl<Tx> InMemoryMempool<Tx> {
    pub fn new(config: MempoolConfig, tx_validator: Arc<dyn CanValidateTransactions<Tx>>) -> Self {
        InMemoryMempool {
            config,
            inner: parking_lot::RwLock::new(MempoolInner::default()),
            notify: Notify::new(),
            tx_validator,
        }
    }

    pub fn from_config(config: MempoolConfig) -> Self {
        Self::new(config, Arc::new(DefaultCanValidateTransactions))
    }
}

/// A default transactions validator.
#[derive(Clone, Debug, Default)]
pub struct DefaultCanValidateTransactions;

impl<Tx> CanValidateTransactions<Tx> for DefaultCanValidateTransactions {
    fn validate_transaction(&self, _tx: Tx) -> Result<(), TransactionValidationError> {
        Ok(())
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
            next_seq: 1,
            entries_by_id: Default::default(),
            entries_by_seq: Default::default(),
        }
    }
}

impl<Tx: Encode<()> + Clone> MempoolInner<Tx> {
    /// Inserts a new transaction into the mempool.
    /// The transaction id is a hash of the transaction body.
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

        let entry = MempoolEntry {
            seq_no,
            tx_id: tx_id.clone(),
            tx,
            origin: tx_origin,
        };

        self.entries_by_id.insert(tx_id.clone(), entry);
        self.entries_by_seq.insert(seq_no, tx_id.clone());
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
                if let Some(entry) = self.entries_by_id.get(tx_id) {
                    let tx_size = to_cbor(&entry.tx).len() as u32;
                    (tx_id.clone(), tx_size, *seq)
                } else {
                    panic!(
                        "Inconsistent mempool state: entry missing for tx_id {:?}",
                        tx_id
                    );
                }
            })
            .collect();
        result.sort_by_key(|(_, _, seq_no)| *seq_no);
        result
    }

    /// Retrieves transactions for the given ids, sorted by their sequence number.
    fn get_txs_for_ids(&self, ids: &[TxId]) -> Vec<Tx> {
        // Make sure that the result are sorted by seq_no
        let mut result: Vec<(&TxId, &MempoolEntry<Tx>)> = self
            .entries_by_id
            .iter()
            .filter(|(key, _)| ids.contains(*key))
            .collect();
        result.sort_by_key(|(_, entry)| entry.seq_no);
        result
            .into_iter()
            .map(|(_, entry)| entry.tx.clone())
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct MempoolEntry<Tx> {
    seq_no: MempoolSeqNo,
    tx_id: TxId,
    tx: Tx,
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

impl<Tx: Send + Sync + 'static> CanValidateTransactions<Tx> for InMemoryMempool<Tx> {
    fn validate_transaction(&self, tx: Tx) -> Result<(), TransactionValidationError> {
        self.tx_validator.validate_transaction(tx)
    }
}

impl<Tx: Send + Sync + 'static + Encode<()> + Clone> TxSubmissionMempool<Tx>
    for InMemoryMempool<Tx>
{
    fn insert(&self, tx: Tx, tx_origin: TxOrigin) -> Result<(TxId, MempoolSeqNo), TxRejectReason> {
        let res = self.inner.write().insert(&self.config, tx, tx_origin);
        if res.is_ok() {
            self.notify.notify_waiters();
        }
        res
    }

    fn get_tx(&self, tx_id: &TxId) -> Option<Tx> {
        self.inner.read().get_tx(tx_id)
    }

    fn tx_ids_since(&self, from_seq: MempoolSeqNo, limit: u16) -> Vec<(TxId, u32, MempoolSeqNo)> {
        self.inner.read().tx_ids_since(from_seq, limit)
    }

    fn wait_for_at_least(
        &self,
        seq_no: MempoolSeqNo,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
        Box::pin(async move {
            loop {
                // TODO: make sure that transactions are valid before returning
                // So we can make sure to send enough valid transactions upstream
                let notified = self.notify.notified();
                let current_seq = { self.inner.read().next_seq };
                if current_seq >= seq_no.0 {
                    return true;
                }

                // Wait until someone inserts a new transaction and notifies us
                notified.await;
            }
        })
    }

    fn get_txs_for_ids(&self, ids: &[TxId]) -> Vec<Tx> {
        self.inner.read().get_txs_for_ids(ids)
    }

    fn last_seq_no(&self) -> MempoolSeqNo {
        MempoolSeqNo(self.inner.read().next_seq - 1)
    }
}

impl<Tx: Send + Sync + 'static + Encode<()> + Clone> Mempool<Tx> for InMemoryMempool<Tx> {
    fn take(&self) -> Vec<Tx> {
        let mut inner = self.inner.write();
        let entries = mem::take(&mut inner.entries_by_id);
        let _ = mem::take(&mut inner.entries_by_seq);
        entries.into_values().map(|entry| entry.tx).collect()
    }

    fn acknowledge<TxKey: Ord, I>(&self, tx: &Tx, keys: fn(&Tx) -> I)
    where
        I: IntoIterator<Item = TxKey>,
        Self: Sized,
    {
        let keys_to_remove = BTreeSet::from_iter(keys(tx));
        let mut inner = self.inner.write();

        // remove entries matching the keys criteria in both maps
        let seq_nos_to_remove: Vec<MempoolSeqNo> = inner
            .entries_by_id
            .values()
            .filter(|entry| {
                keys(&entry.tx)
                    .into_iter()
                    .any(|k| keys_to_remove.contains(&k))
            })
            .map(|entry| entry.seq_no)
            .collect();
        inner.entries_by_id.retain(|_, entry| {
            !keys(&entry.tx)
                .into_iter()
                .any(|k| keys_to_remove.contains(&k))
        });
        for seq_no in seq_nos_to_remove {
            inner.entries_by_seq.remove(&seq_no);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use amaru_kernel::peer::Peer;
    use assertables::assert_some_eq_x;
    use minicbor::Decode;
    use std::ops::Deref;
    use std::slice;
    use std::str::FromStr;
    use std::time::Duration;
    use tokio::time::timeout;

    #[tokio::test]
    async fn insert_a_transaction() -> anyhow::Result<()> {
        let mempool = InMemoryMempool::from_config(MempoolConfig::default().with_max_txs(5));
        let tx = Tx::from_str("tx1").unwrap();
        let (tx_id, seq_nb) = mempool
            .insert(tx.clone(), TxOrigin::Remote(Peer::new("peer-1")))
            .unwrap();

        assert_some_eq_x!(mempool.get_tx(&tx_id), tx.clone());
        assert_eq!(
            mempool.get_txs_for_ids(slice::from_ref(&tx_id)),
            vec![tx.clone()]
        );
        assert_eq!(mempool.tx_ids_since(seq_nb, 100), vec![(tx_id, 5, seq_nb)]);
        assert!(
            mempool.wait_for_at_least(seq_nb).await,
            "should have at least seq no"
        );
        assert!(
            timeout(
                Duration::from_millis(100),
                mempool.wait_for_at_least(seq_nb.add(100))
            )
            .await
            .is_err(),
            "should timeout waiting for a seq no that is too high"
        );
        Ok(())
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
