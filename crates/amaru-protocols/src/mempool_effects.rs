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

use amaru_kernel::{Transaction, TransactionId, cbor::WithOriginalBytes};
use amaru_ouroboros::ResourceMempool;
use amaru_ouroboros_traits::{MempoolSeqNo, MempoolState, TxInsertResult, TxOrigin, TxSubmissionMempool};
use amaru_pure_stage::{BoxFuture, Effects, ExternalEffectAPI, Resources, SendData, Void};
use serde::{Deserialize, Serialize};

/// Implementation of Mempool effects using amaru_pure_stage::Effects.
///
/// It supports operations
///
/// - for the tx submission protocol
///
#[derive(Clone)]
pub struct MemoryPool {
    effects: Effects<Void>,
}

pub trait AsyncMempool: Send + Sync {
    fn insert(&self, tx: WithOriginalBytes<Transaction>, tx_origin: TxOrigin) -> BoxFuture<'_, TxInsertResult>;
    fn get_tx(&self, tx_id: TransactionId) -> BoxFuture<'_, Option<WithOriginalBytes<Transaction>>>;
    fn contains(&self, tx_id: &TransactionId) -> BoxFuture<'_, bool>;
    fn tx_ids_since(
        &self,
        from_seq: MempoolSeqNo,
        limit: u16,
    ) -> BoxFuture<'_, Vec<(TransactionId, u32, MempoolSeqNo)>>;
    fn get_txs_for_ids(&self, ids: &[TransactionId]) -> BoxFuture<'_, Vec<WithOriginalBytes<Transaction>>>;
    fn mempool_txs(&self) -> BoxFuture<'_, Vec<WithOriginalBytes<Transaction>>>;
    fn remove_txs(&self, ids: &[TransactionId]) -> BoxFuture<'_, ()>;
    fn last_seq_no(&self) -> BoxFuture<'_, MempoolSeqNo>;
    fn is_near_capacity(&self, additional_bytes: u64) -> BoxFuture<'_, bool>;
    fn state(&self) -> BoxFuture<'_, MempoolState>;
}

impl MemoryPool {
    pub fn new<T: SendData + Sync + 'static>(effects: Effects<T>) -> MemoryPool {
        MemoryPool { effects: effects.erase() }
    }

    pub fn external<E: ExternalEffectAPI + 'static>(&self, effect: E) -> BoxFuture<'static, E::Response> {
        self.effects.external(effect)
    }

    pub fn insert(
        &self,
        tx: WithOriginalBytes<Transaction>,
        tx_origin: TxOrigin,
    ) -> BoxFuture<'static, TxInsertResult> {
        self.external(Insert::new(tx, tx_origin))
    }

    pub fn get_tx(&self, tx_id: &TransactionId) -> BoxFuture<'static, Option<WithOriginalBytes<Transaction>>> {
        self.external(GetTx::new(*tx_id))
    }

    pub fn contains(&self, tx_id: &TransactionId) -> BoxFuture<'static, bool> {
        self.external(ContainsTx::new(*tx_id))
    }

    pub fn tx_ids_since(
        &self,
        from_seq: MempoolSeqNo,
        limit: u16,
    ) -> BoxFuture<'static, Vec<(TransactionId, u32, MempoolSeqNo)>> {
        self.external(TxIdsSince::new(from_seq, limit))
    }

    pub fn get_txs_for_ids(&self, ids: &[TransactionId]) -> BoxFuture<'static, Vec<WithOriginalBytes<Transaction>>> {
        self.external(GetTxsForIds::new(ids))
    }

    pub fn mempool_txs(&self) -> BoxFuture<'static, Vec<WithOriginalBytes<Transaction>>> {
        self.external(MempoolTxs)
    }

    pub fn remove_txs(&self, ids: &[TransactionId]) -> BoxFuture<'static, ()> {
        self.external(RemoveTxs::new(ids))
    }

    /// This effect gets the last assigned sequence number in the mempool.
    pub fn last_seq_no(&self) -> BoxFuture<'static, MempoolSeqNo> {
        self.external(LastSeqNo)
    }

    /// This effect returns whether the mempool would be over its configured maximum byte size if accepting
    /// a transaction of size `additional_bytes`.
    pub fn is_near_capacity(&self, additional_bytes: u64) -> BoxFuture<'static, bool> {
        self.external(IsNearCapacity { additional_bytes })
    }

    /// This effect retrieves a snapshot of the mempool's tx count and cumulative size.
    pub fn state(&self) -> BoxFuture<'static, MempoolState> {
        self.external(State)
    }
}

impl AsyncMempool for MemoryPool {
    fn insert(&self, tx: WithOriginalBytes<Transaction>, tx_origin: TxOrigin) -> BoxFuture<'_, TxInsertResult> {
        MemoryPool::insert(self, tx, tx_origin)
    }

    fn get_tx(&self, tx_id: TransactionId) -> BoxFuture<'_, Option<WithOriginalBytes<Transaction>>> {
        MemoryPool::get_tx(self, &tx_id)
    }

    fn contains(&self, tx_id: &TransactionId) -> BoxFuture<'_, bool> {
        MemoryPool::contains(self, tx_id)
    }

    fn tx_ids_since(
        &self,
        from_seq: MempoolSeqNo,
        limit: u16,
    ) -> BoxFuture<'_, Vec<(TransactionId, u32, MempoolSeqNo)>> {
        MemoryPool::tx_ids_since(self, from_seq, limit)
    }

    fn get_txs_for_ids(&self, ids: &[TransactionId]) -> BoxFuture<'_, Vec<WithOriginalBytes<Transaction>>> {
        MemoryPool::get_txs_for_ids(self, ids)
    }

    fn mempool_txs(&self) -> BoxFuture<'_, Vec<WithOriginalBytes<Transaction>>> {
        MemoryPool::mempool_txs(self)
    }

    fn remove_txs(&self, ids: &[TransactionId]) -> BoxFuture<'_, ()> {
        MemoryPool::remove_txs(self, ids)
    }

    fn last_seq_no(&self) -> BoxFuture<'_, MempoolSeqNo> {
        MemoryPool::last_seq_no(self)
    }

    fn is_near_capacity(&self, additional_bytes: u64) -> BoxFuture<'_, bool> {
        MemoryPool::is_near_capacity(self, additional_bytes)
    }

    fn state(&self) -> BoxFuture<'_, MempoolState> {
        MemoryPool::state(self)
    }
}

impl<T: TxSubmissionMempool<WithOriginalBytes<Transaction>> + ?Sized> AsyncMempool for T {
    fn insert(&self, tx: WithOriginalBytes<Transaction>, tx_origin: TxOrigin) -> BoxFuture<'_, TxInsertResult> {
        Box::pin(async move { TxSubmissionMempool::insert(self, tx, tx_origin) })
    }

    fn get_tx(&self, tx_id: TransactionId) -> BoxFuture<'_, Option<WithOriginalBytes<Transaction>>> {
        Box::pin(async move { TxSubmissionMempool::get_tx(self, &tx_id) })
    }

    fn contains(&self, tx_id: &TransactionId) -> BoxFuture<'_, bool> {
        let tx_id = *tx_id;
        Box::pin(async move { TxSubmissionMempool::contains(self, &tx_id) })
    }

    fn tx_ids_since(
        &self,
        from_seq: MempoolSeqNo,
        limit: u16,
    ) -> BoxFuture<'_, Vec<(TransactionId, u32, MempoolSeqNo)>> {
        Box::pin(async move { TxSubmissionMempool::tx_ids_since(self, from_seq, limit) })
    }

    fn get_txs_for_ids(&self, ids: &[TransactionId]) -> BoxFuture<'_, Vec<WithOriginalBytes<Transaction>>> {
        let tx_ids = ids.to_vec();
        Box::pin(async move { TxSubmissionMempool::get_txs_for_ids(self, &tx_ids) })
    }

    fn mempool_txs(&self) -> BoxFuture<'_, Vec<WithOriginalBytes<Transaction>>> {
        Box::pin(async move { TxSubmissionMempool::mempool_txs(self) })
    }

    fn remove_txs(&self, ids: &[TransactionId]) -> BoxFuture<'_, ()> {
        let tx_ids = ids.to_vec();
        Box::pin(async move { TxSubmissionMempool::remove_txs(self, &tx_ids) })
    }

    fn last_seq_no(&self) -> BoxFuture<'_, MempoolSeqNo> {
        Box::pin(async move { TxSubmissionMempool::last_seq_no(self) })
    }

    fn is_near_capacity(&self, additional_bytes: u64) -> BoxFuture<'_, bool> {
        Box::pin(async move { TxSubmissionMempool::is_near_capacity(self, additional_bytes) })
    }

    fn state(&self) -> BoxFuture<'_, MempoolState> {
        Box::pin(async move { TxSubmissionMempool::state(self) })
    }
}

// EXTERNAL EFFECTS DEFINITIONS

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Insert {
    tx: WithOriginalBytes<Transaction>,
    tx_origin: TxOrigin,
}

impl Insert {
    pub fn new(tx: WithOriginalBytes<Transaction>, tx_origin: TxOrigin) -> Self {
        Self { tx, tx_origin }
    }
}

impl ExternalEffectAPI for Insert {
    type Response = TxInsertResult;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let mempool = resources.get::<ResourceMempool<Transaction>>().expect("ResourceMempool requires a mempool");
            mempool.insert(self.tx.clone(), self.tx_origin.clone())
        })
    }
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct GetTx {
    tx_id: TransactionId,
}

impl GetTx {
    pub fn new(tx_id: TransactionId) -> Self {
        Self { tx_id }
    }
}

impl ExternalEffectAPI for GetTx {
    type Response = Option<WithOriginalBytes<Transaction>>;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let mempool = resources.get::<ResourceMempool<Transaction>>().expect("ResourceMempool requires a mempool");
            mempool.get_tx(&self.tx_id)
        })
    }
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct ContainsTx {
    tx_id: TransactionId,
}

impl ContainsTx {
    pub fn new(tx_id: TransactionId) -> Self {
        Self { tx_id }
    }
}

impl ExternalEffectAPI for ContainsTx {
    type Response = bool;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let mempool = resources.get::<ResourceMempool<Transaction>>().expect("ResourceMempool requires a mempool");
            mempool.contains(&self.tx_id)
        })
    }
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

impl ExternalEffectAPI for TxIdsSince {
    type Response = Vec<(TransactionId, u32, MempoolSeqNo)>;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let mempool = resources.get::<ResourceMempool<Transaction>>().expect("ResourceMempool requires a mempool");
            mempool.tx_ids_since(self.mempool_seqno, self.limit)
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct GetTxsForIds {
    tx_ids: Vec<TransactionId>,
}

impl GetTxsForIds {
    pub fn new(ids: &[TransactionId]) -> Self {
        Self { tx_ids: ids.to_vec() }
    }
}

impl ExternalEffectAPI for GetTxsForIds {
    type Response = Vec<WithOriginalBytes<Transaction>>;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let mempool = resources.get::<ResourceMempool<Transaction>>().expect("ResourceMempool requires a mempool");
            mempool.get_txs_for_ids(&self.tx_ids)
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct MempoolTxs;

impl ExternalEffectAPI for MempoolTxs {
    type Response = Vec<WithOriginalBytes<Transaction>>;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let mempool = resources.get::<ResourceMempool<Transaction>>().expect("ResourceMempool requires a mempool");
            mempool.mempool_txs()
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct RemoveTxs {
    tx_ids: Vec<TransactionId>,
}

impl RemoveTxs {
    pub fn new(ids: &[TransactionId]) -> Self {
        Self { tx_ids: ids.to_vec() }
    }
}

impl ExternalEffectAPI for RemoveTxs {
    type Response = ();

    #[expect(clippy::expect_used, clippy::unit_arg)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let mempool = resources.get::<ResourceMempool<Transaction>>().expect("ResourceMempool requires a mempool");
            mempool.remove_txs(&self.tx_ids)
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct LastSeqNo;

impl ExternalEffectAPI for LastSeqNo {
    type Response = MempoolSeqNo;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let mempool = resources.get::<ResourceMempool<Transaction>>().expect("ResourceMempool requires a mempool");
            mempool.last_seq_no()
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct IsNearCapacity {
    additional_bytes: u64,
}

impl ExternalEffectAPI for IsNearCapacity {
    type Response = bool;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let mempool = resources.get::<ResourceMempool<Transaction>>().expect("ResourceMempool requires a mempool");
            mempool.is_near_capacity(self.additional_bytes)
        })
    }
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct State;

impl ExternalEffectAPI for State {
    type Response = MempoolState;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let mempool = resources.get::<ResourceMempool<Transaction>>().expect("ResourceMempool requires a mempool");
            mempool.state()
        })
    }
}

#[cfg(test)]
mod tests {
    use amaru_kernel::{
        Transaction, TransactionBody, TransactionId, WitnessSet,
        cbor::{WithOriginalBytes, WithSize},
    };
    use amaru_ouroboros_traits::{MempoolSeqNo, MempoolState, TxInsertResult, TxOrigin, TxSubmissionMempool};

    #[allow(dead_code)]
    pub struct ConstantMempool {
        tx: WithOriginalBytes<Transaction>,
    }

    impl ConstantMempool {
        #[allow(dead_code)]
        pub fn new() -> Self {
            let body = TransactionBody::new([], [], 0);
            let witnesses = WithSize::<WitnessSet>::default().with_size(1);
            let tx = Transaction { body, witnesses, is_expected_valid: true, auxiliary_data: None }.into();
            Self { tx }
        }
    }

    impl TxSubmissionMempool<WithOriginalBytes<Transaction>> for ConstantMempool {
        fn insert(&self, tx: WithOriginalBytes<Transaction>, _tx_origin: TxOrigin) -> TxInsertResult {
            TxInsertResult::accepted(tx.tx_id(), MempoolSeqNo(1))
        }

        fn get_tx(&self, _tx_id: &TransactionId) -> Option<WithOriginalBytes<Transaction>> {
            Some(self.tx.clone())
        }

        fn tx_ids_since(&self, _from_seq: MempoolSeqNo, _limit: u16) -> Vec<(TransactionId, u32, MempoolSeqNo)> {
            vec![(self.tx.tx_id(), 100, MempoolSeqNo(1))]
        }

        fn get_txs_for_ids(&self, _ids: &[TransactionId]) -> Vec<WithOriginalBytes<Transaction>> {
            vec![self.tx.clone()]
        }

        fn mempool_txs(&self) -> Vec<WithOriginalBytes<Transaction>> {
            vec![self.tx.clone()]
        }

        fn remove_txs(&self, _ids: &[TransactionId]) {}

        fn last_seq_no(&self) -> MempoolSeqNo {
            MempoolSeqNo(1)
        }

        fn is_near_capacity(&self, _additional_bytes: u64) -> bool {
            false
        }

        fn state(&self) -> MempoolState {
            MempoolState { tx_count: 1, size_bytes: 0 }
        }
    }
}
