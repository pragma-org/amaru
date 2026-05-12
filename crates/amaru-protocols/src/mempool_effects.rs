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

use std::fmt::Debug;

use amaru_kernel::Transaction;
use amaru_ouroboros::ResourceMempool;
use amaru_ouroboros_traits::{MempoolError, MempoolSeqNo, TxId, TxInsertResult, TxOrigin, TxSubmissionMempool};
use pure_stage::{BoxFuture, Effects, ExternalEffect, ExternalEffectAPI, Resources, SendData};
use serde::{Deserialize, Serialize};

/// Implementation of Mempool effects using pure_stage::Effects.
///
/// It supports operations
///
/// - for the tx submission protocol
///
#[derive(Clone)]
pub struct MemoryPool<T> {
    effects: Effects<T>,
}

pub trait AsyncMempool: Send + Sync {
    fn insert(&self, tx: Transaction, tx_origin: TxOrigin) -> BoxFuture<'_, Result<TxInsertResult, MempoolError>>;
    fn get_tx(&self, tx_id: TxId) -> BoxFuture<'_, Option<Transaction>>;
    fn contains(&self, tx_id: TxId) -> BoxFuture<'_, bool>;
    fn tx_ids_since(&self, from_seq: MempoolSeqNo, limit: u16) -> BoxFuture<'_, Vec<(TxId, u32, MempoolSeqNo)>>;
    fn wait_for_at_least(&self, seq_no: MempoolSeqNo) -> BoxFuture<'_, bool>;
    fn get_txs_for_ids(&self, ids: Vec<TxId>) -> BoxFuture<'_, Vec<Transaction>>;
}

impl<T: SendData + Sync + 'static> MemoryPool<T> {
    pub fn new(effects: Effects<T>) -> MemoryPool<T> {
        MemoryPool { effects }
    }

    pub fn external<E: ExternalEffectAPI + 'static>(&self, effect: E) -> BoxFuture<'static, E::Response> {
        self.effects.external(effect)
    }

    pub fn insert(
        &self,
        tx: Transaction,
        tx_origin: TxOrigin,
    ) -> BoxFuture<'static, Result<TxInsertResult, MempoolError>> {
        self.external(Insert::new(tx, tx_origin))
    }

    pub fn get_tx(&self, tx_id: &TxId) -> BoxFuture<'static, Option<Transaction>> {
        self.external(GetTx::new(*tx_id))
    }

    pub fn contains(&self, tx_id: &TxId) -> BoxFuture<'static, bool> {
        self.external(ContainsTx::new(*tx_id))
    }

    pub fn tx_ids_since(
        &self,
        from_seq: MempoolSeqNo,
        limit: u16,
    ) -> BoxFuture<'static, Vec<(TxId, u32, MempoolSeqNo)>> {
        self.external(TxIdsSince::new(from_seq, limit))
    }

    pub fn wait_for_at_least(&self, seq_no: MempoolSeqNo) -> BoxFuture<'static, bool> {
        let mempool = MemoryPool::new(self.effects.clone());
        Box::pin(async move { mempool.last_seq_no().await >= seq_no })
    }

    pub fn get_txs_for_ids(&self, ids: &[TxId]) -> BoxFuture<'static, Vec<Transaction>> {
        self.external(GetTxsForIds::new(ids))
    }

    pub fn last_seq_no(&self) -> BoxFuture<'static, MempoolSeqNo> {
        self.external(LastSeqNo)
    }
}

impl<T: SendData + Sync + 'static> AsyncMempool for MemoryPool<T> {
    fn insert(&self, tx: Transaction, tx_origin: TxOrigin) -> BoxFuture<'_, Result<TxInsertResult, MempoolError>> {
        MemoryPool::insert(self, tx, tx_origin)
    }

    fn get_tx(&self, tx_id: TxId) -> BoxFuture<'_, Option<Transaction>> {
        MemoryPool::get_tx(self, &tx_id)
    }

    fn contains(&self, tx_id: TxId) -> BoxFuture<'_, bool> {
        MemoryPool::contains(self, &tx_id)
    }

    fn tx_ids_since(&self, from_seq: MempoolSeqNo, limit: u16) -> BoxFuture<'_, Vec<(TxId, u32, MempoolSeqNo)>> {
        MemoryPool::tx_ids_since(self, from_seq, limit)
    }

    fn wait_for_at_least(&self, seq_no: MempoolSeqNo) -> BoxFuture<'_, bool> {
        MemoryPool::wait_for_at_least(self, seq_no)
    }

    fn get_txs_for_ids(&self, ids: Vec<TxId>) -> BoxFuture<'_, Vec<Transaction>> {
        MemoryPool::get_txs_for_ids(self, &ids)
    }
}

impl<T: TxSubmissionMempool<Transaction> + ?Sized> AsyncMempool for T {
    fn insert(&self, tx: Transaction, tx_origin: TxOrigin) -> BoxFuture<'_, Result<TxInsertResult, MempoolError>> {
        Box::pin(async move { TxSubmissionMempool::insert(self, tx, tx_origin) })
    }

    fn get_tx(&self, tx_id: TxId) -> BoxFuture<'_, Option<Transaction>> {
        Box::pin(async move { TxSubmissionMempool::get_tx(self, &tx_id) })
    }

    fn contains(&self, tx_id: TxId) -> BoxFuture<'_, bool> {
        Box::pin(async move { TxSubmissionMempool::contains(self, &tx_id) })
    }

    fn tx_ids_since(&self, from_seq: MempoolSeqNo, limit: u16) -> BoxFuture<'_, Vec<(TxId, u32, MempoolSeqNo)>> {
        Box::pin(async move { TxSubmissionMempool::tx_ids_since(self, from_seq, limit) })
    }

    fn wait_for_at_least(&self, seq_no: MempoolSeqNo) -> BoxFuture<'_, bool> {
        Box::pin(async move { TxSubmissionMempool::last_seq_no(self) >= seq_no })
    }

    fn get_txs_for_ids(&self, ids: Vec<TxId>) -> BoxFuture<'_, Vec<Transaction>> {
        Box::pin(async move { TxSubmissionMempool::get_txs_for_ids(self, &ids) })
    }
}

// EXTERNAL EFFECTS DEFINITIONS

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Insert {
    tx: Transaction,
    tx_origin: TxOrigin,
}

impl Insert {
    pub fn new(tx: Transaction, tx_origin: TxOrigin) -> Self {
        Self { tx, tx_origin }
    }
}

impl ExternalEffect for Insert {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let mempool = resources.get::<ResourceMempool<Transaction>>().expect("ResourceMempool requires a mempool");
            mempool.insert(self.tx, self.tx_origin)
        })
    }
}

impl ExternalEffectAPI for Insert {
    type Response = Result<TxInsertResult, MempoolError>;
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct GetTx {
    tx_id: TxId,
}

impl GetTx {
    pub fn new(tx_id: TxId) -> Self {
        Self { tx_id }
    }
}

impl ExternalEffect for GetTx {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let mempool = resources.get::<ResourceMempool<Transaction>>().expect("ResourceMempool requires a mempool");
            mempool.get_tx(&self.tx_id)
        })
    }
}

impl ExternalEffectAPI for GetTx {
    type Response = Option<Transaction>;
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct ContainsTx {
    tx_id: TxId,
}

impl ContainsTx {
    pub fn new(tx_id: TxId) -> Self {
        Self { tx_id }
    }
}

impl ExternalEffect for ContainsTx {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let mempool = resources.get::<ResourceMempool<Transaction>>().expect("ResourceMempool requires a mempool");
            mempool.contains(&self.tx_id)
        })
    }
}

impl ExternalEffectAPI for ContainsTx {
    type Response = bool;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct TxIdsSince {
    mempool_seqno: MempoolSeqNo,
    limit: u16,
}

impl TxIdsSince {
    pub fn new(mempool_seqno: MempoolSeqNo, limit: u16) -> Self {
        Self { mempool_seqno, limit }
    }
}

impl ExternalEffect for TxIdsSince {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let mempool = resources.get::<ResourceMempool<Transaction>>().expect("ResourceMempool requires a mempool");
            mempool.tx_ids_since(self.mempool_seqno, self.limit)
        })
    }
}

impl ExternalEffectAPI for TxIdsSince {
    type Response = Vec<(TxId, u32, MempoolSeqNo)>;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct GetTxsForIds {
    tx_ids: Vec<TxId>,
}

impl GetTxsForIds {
    pub fn new(ids: &[TxId]) -> Self {
        Self { tx_ids: ids.to_vec() }
    }
}

impl ExternalEffect for GetTxsForIds {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let mempool = resources.get::<ResourceMempool<Transaction>>().expect("ResourceMempool requires a mempool");
            mempool.get_txs_for_ids(&self.tx_ids)
        })
    }
}

impl ExternalEffectAPI for GetTxsForIds {
    type Response = Vec<Transaction>;
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct LastSeqNo;

impl ExternalEffect for LastSeqNo {
    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap_sync({
            let mempool = resources.get::<ResourceMempool<Transaction>>().expect("ResourceMempool requires a mempool");
            mempool.last_seq_no()
        })
    }
}

impl ExternalEffectAPI for LastSeqNo {
    type Response = MempoolSeqNo;
}

#[cfg(test)]
mod tests {
    use amaru_kernel::{Transaction, TransactionBody, WitnessSet};
    use amaru_ouroboros_traits::{MempoolError, MempoolSeqNo, TxId, TxInsertResult, TxOrigin, TxSubmissionMempool};

    #[allow(dead_code)]
    pub struct ConstantMempool {
        tx: Transaction,
    }

    impl ConstantMempool {
        #[allow(dead_code)]
        pub fn new() -> Self {
            let body = TransactionBody::new([], [], 0);
            let witnesses = WitnessSet::default();
            let tx: Transaction = Transaction { body, witnesses, is_expected_valid: true, auxiliary_data: None };
            Self { tx }
        }
    }

    impl TxSubmissionMempool<Transaction> for ConstantMempool {
        fn insert(&self, tx: Transaction, _tx_origin: TxOrigin) -> Result<TxInsertResult, MempoolError> {
            Ok(TxInsertResult::accepted(TxId::from(&tx), MempoolSeqNo(1)))
        }

        fn get_tx(&self, _tx_id: &TxId) -> Option<Transaction> {
            Some(self.tx.clone())
        }

        fn tx_ids_since(&self, _from_seq: MempoolSeqNo, _limit: u16) -> Vec<(TxId, u32, MempoolSeqNo)> {
            vec![(TxId::from(&self.tx), 100, MempoolSeqNo(1))]
        }

        fn get_txs_for_ids(&self, _ids: &[TxId]) -> Vec<Transaction> {
            vec![self.tx.clone()]
        }

        fn last_seq_no(&self) -> MempoolSeqNo {
            MempoolSeqNo(1)
        }
    }
}
