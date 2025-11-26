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

use amaru_ouroboros_traits::{Mempool, MempoolSeqNo, TxId, TxOrigin, TxRejectReason};
use minicbor::Encode;
use parking_lot::RwLock;
use std::pin::Pin;
use std::sync::Arc;
use std::{collections::BTreeSet, mem};

#[derive(Debug, Default)]
pub struct DummyMempool<T> {
    inner: RwLock<DummyMempoolInner<T>>,
}

impl<T> DummyMempool<T> {
    pub fn new() -> Self {
        DummyMempool {
            inner: RwLock::new(DummyMempoolInner::new()),
        }
    }
}

#[derive(Debug, Default)]
pub struct DummyMempoolInner<T> {
    transactions: Vec<T>,
}

impl<Tx: Encode<()> + Send + Sync + 'static> Mempool<Tx> for DummyMempool<Tx> {
    fn insert(&self, tx: Tx, _tx_origin: TxOrigin) -> Result<(TxId, MempoolSeqNo), TxRejectReason> {
        let tx_id = TxId::from(&tx);
        self.inner.write().transactions.push(tx);
        Ok((
            tx_id,
            MempoolSeqNo(self.inner.read().transactions.len() as u64 - 1),
        ))
    }

    fn take(&self) -> Vec<Tx> {
        mem::take(&mut self.inner.write().transactions)
    }

    fn acknowledge<TxKey: Ord, I>(&self, tx: &Tx, keys: fn(&Tx) -> I)
    where
        I: IntoIterator<Item = TxKey>,
        Self: Sized,
    {
        let keys_to_remove = BTreeSet::from_iter(keys(tx));
        self.inner
            .write()
            .transactions
            .retain(|tx| !keys(tx).into_iter().any(|k| keys_to_remove.contains(&k)));
    }

    fn get_tx(&self, _tx_id: &TxId) -> Option<Arc<Tx>> {
        None
    }

    fn tx_ids_since(&self, _from_seq: MempoolSeqNo, _limit: u16) -> Vec<(TxId, u32, MempoolSeqNo)> {
        vec![]
    }

    fn wait_for_at_least(
        &self,
        _seq_no: MempoolSeqNo,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
        Box::pin(async move { true })
    }

    fn get_txs_for_ids(&self, _ids: &[TxId]) -> Vec<Arc<Tx>> {
        vec![]
    }

    fn last_seq_no(&self) -> MempoolSeqNo {
        MempoolSeqNo(self.inner.read().transactions.len() as u64)
    }
}

impl<T> DummyMempoolInner<T> {
    pub fn new() -> Self {
        DummyMempoolInner {
            transactions: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use minicbor::Encoder;
    use minicbor::encode::{Error, Write};

    #[test]
    fn take_empty() {
        let mempool: DummyMempool<FakeTx<'_>> = DummyMempool::new();
        assert_eq!(mempool.take(), vec![]);
    }

    #[test]
    fn add_then_take() {
        let mempool = DummyMempool::new();
        mempool.add(FakeTx::new("tx1", &[1])).unwrap();
        mempool.add(FakeTx::new("tx2", &[2])).unwrap();
        assert_eq!(
            mempool.take(),
            vec![FakeTx::new("tx1", &[1]), FakeTx::new("tx2", &[2])]
        );
    }

    #[test]
    fn invalidate_entries() {
        let mempool = DummyMempool::new();
        mempool.add(FakeTx::new("tx1", &[1, 2])).unwrap();
        mempool.add(FakeTx::new("tx2", &[3, 4])).unwrap();
        mempool.add(FakeTx::new("tx3", &[5, 6])).unwrap();
        mempool.acknowledge(&FakeTx::new("tx4", &[2, 5, 7]), FakeTx::keys);
        assert_eq!(mempool.take(), vec![FakeTx::new("tx2", &[3, 4])]);
    }

    // HELPERS

    #[derive(Debug, PartialEq, Eq)]
    struct FakeTx<'a> {
        id: &'a str,
        inputs: Vec<usize>,
    }

    impl Encode<()> for FakeTx<'_> {
        fn encode<W: Write>(
            &self,
            e: &mut Encoder<W>,
            _ctx: &mut (),
        ) -> Result<(), Error<W::Error>> {
            e.encode(self.id)?;
            e.encode(&self.inputs)?;
            Ok(())
        }
    }

    impl<'a> FakeTx<'a> {
        fn new(id: &'a str, inputs: &'_ [usize]) -> Self {
            FakeTx {
                id,
                inputs: Vec::from(inputs),
            }
        }

        fn keys(&self) -> Vec<usize> {
            self.inputs.clone()
        }
    }
}
