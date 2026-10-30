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
use amaru_ouroboros_traits::{Mempool, MempoolSeqNo, MempoolState, TxInsertResult, TxOrigin};
use amaru_pure_stage::{BoxFuture, DurationDist, Effects, ExternalEffectAPI, Resources, SendData, Void};
use serde::{Deserialize, Serialize};

use crate::effect_timings;

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

/// Adapts stage effects to the asynchronous mempool interface without selecting a storage
/// implementation. The concrete mempool is resolved through the stage resource registry when an
/// effect is executed.
#[derive(Clone)]
pub struct MempoolEffects {
    effects: Effects<Void>,
}

impl MempoolEffects {
    pub fn new<T: SendData + Sync + 'static>(effects: Effects<T>) -> Self {
        Self { effects: effects.erase() }
    }
}

impl AsyncMempool for MempoolEffects {
    fn insert(&self, tx: WithOriginalBytes<Transaction>, tx_origin: TxOrigin) -> BoxFuture<'_, TxInsertResult> {
        self.effects.external(Insert::new(tx, tx_origin))
    }

    fn get_tx(&self, tx_id: TransactionId) -> BoxFuture<'_, Option<WithOriginalBytes<Transaction>>> {
        self.effects.external(GetTx::new(tx_id))
    }

    fn contains(&self, tx_id: &TransactionId) -> BoxFuture<'_, bool> {
        self.effects.external(ContainsTx::new(*tx_id))
    }

    fn tx_ids_since(
        &self,
        from_seq: MempoolSeqNo,
        limit: u16,
    ) -> BoxFuture<'_, Vec<(TransactionId, u32, MempoolSeqNo)>> {
        self.effects.external(TxIdsSince::new(from_seq, limit))
    }

    fn get_txs_for_ids(&self, ids: &[TransactionId]) -> BoxFuture<'_, Vec<WithOriginalBytes<Transaction>>> {
        self.effects.external(GetTxsForIds::new(ids))
    }

    fn mempool_txs(&self) -> BoxFuture<'_, Vec<WithOriginalBytes<Transaction>>> {
        self.effects.external(MempoolTxs)
    }

    fn remove_txs(&self, ids: &[TransactionId]) -> BoxFuture<'_, ()> {
        self.effects.external(RemoveTxs::new(ids))
    }

    fn last_seq_no(&self) -> BoxFuture<'_, MempoolSeqNo> {
        self.effects.external(LastSeqNo)
    }

    fn is_near_capacity(&self, additional_bytes: u64) -> BoxFuture<'_, bool> {
        self.effects.external(IsNearCapacity { additional_bytes })
    }

    fn state(&self) -> BoxFuture<'_, MempoolState> {
        self.effects.external(State)
    }
}

impl<T: Mempool<WithOriginalBytes<Transaction>> + ?Sized> AsyncMempool for T {
    fn insert(&self, tx: WithOriginalBytes<Transaction>, tx_origin: TxOrigin) -> BoxFuture<'_, TxInsertResult> {
        Box::pin(async move { Mempool::insert(self, tx, tx_origin) })
    }

    fn get_tx(&self, tx_id: TransactionId) -> BoxFuture<'_, Option<WithOriginalBytes<Transaction>>> {
        Box::pin(async move { Mempool::get_tx(self, &tx_id) })
    }

    fn contains(&self, tx_id: &TransactionId) -> BoxFuture<'_, bool> {
        let tx_id = *tx_id;
        Box::pin(async move { Mempool::contains(self, &tx_id) })
    }

    fn tx_ids_since(
        &self,
        from_seq: MempoolSeqNo,
        limit: u16,
    ) -> BoxFuture<'_, Vec<(TransactionId, u32, MempoolSeqNo)>> {
        Box::pin(async move { Mempool::tx_ids_since(self, from_seq, limit) })
    }

    fn get_txs_for_ids(&self, ids: &[TransactionId]) -> BoxFuture<'_, Vec<WithOriginalBytes<Transaction>>> {
        let tx_ids = ids.to_vec();
        Box::pin(async move { Mempool::get_txs_for_ids(self, &tx_ids) })
    }

    fn mempool_txs(&self) -> BoxFuture<'_, Vec<WithOriginalBytes<Transaction>>> {
        Box::pin(async move { Mempool::mempool_txs(self) })
    }

    fn remove_txs(&self, ids: &[TransactionId]) -> BoxFuture<'_, ()> {
        let tx_ids = ids.to_vec();
        Box::pin(async move { Mempool::remove_txs(self, &tx_ids) })
    }

    fn last_seq_no(&self) -> BoxFuture<'_, MempoolSeqNo> {
        Box::pin(async move { Mempool::last_seq_no(self) })
    }

    fn is_near_capacity(&self, additional_bytes: u64) -> BoxFuture<'_, bool> {
        Box::pin(async move { Mempool::is_near_capacity(self, additional_bytes) })
    }

    fn state(&self) -> BoxFuture<'_, MempoolState> {
        Box::pin(async move { Mempool::state(self) })
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
    // const SIMULATED_DURATION: DurationDist = effect_timings::mempool_effects__Insert;

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
    // const SIMULATED_DURATION: DurationDist = effect_timings::mempool_effects__GetTx;

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
    // const SIMULATED_DURATION: DurationDist = effect_timings::mempool_effects__ContainsTx;

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
    // const SIMULATED_DURATION: DurationDist = effect_timings::mempool_effects__TxIdsSince;

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
    // const SIMULATED_DURATION: DurationDist = effect_timings::mempool_effects__GetTxsForIds;

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
    const SIMULATED_DURATION: DurationDist = effect_timings::mempool_effects__MempoolTxs;

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
    // const SIMULATED_DURATION: DurationDist = effect_timings::mempool_effects__RemoveTxs;

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
    const SIMULATED_DURATION: DurationDist = effect_timings::mempool_effects__LastSeqNo;

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
    // const SIMULATED_DURATION: DurationDist = effect_timings::mempool_effects__IsNearCapacity;

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
    const SIMULATED_DURATION: DurationDist = effect_timings::mempool_effects__State;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let mempool = resources.get::<ResourceMempool<Transaction>>().expect("ResourceMempool requires a mempool");
            mempool.state()
        })
    }
}
