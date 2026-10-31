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
    /// Insert a transaction, preserving the origin used for relay policy.
    fn insert(&self, tx: WithOriginalBytes<Transaction>, tx_origin: TxOrigin) -> BoxFuture<'_, TxInsertResult>;

    /// Retrieve transactions by id in insertion order, omitting unknown ids.
    fn get(&self, ids: &[TransactionId]) -> BoxFuture<'_, Vec<WithOriginalBytes<Transaction>>>;

    /// Return whether a transaction with the given id is currently active.
    fn contains(&self, tx_id: &TransactionId) -> BoxFuture<'_, bool>;

    /// Return transaction ids from `from_seq` (inclusive), in insertion order.
    fn slice(&self, from_seq: MempoolSeqNo, limit: u16) -> BoxFuture<'_, Vec<(TransactionId, u32, MempoolSeqNo)>>;

    /// Return a cloned snapshot in insertion order. The async boundary cannot expose borrowed
    /// transactions from the underlying mempool read view.
    fn iter(&self) -> BoxFuture<'_, Vec<WithOriginalBytes<Transaction>>>;

    /// Remove the identified transactions from the active relay set.
    fn remove(&self, ids: &[TransactionId]) -> BoxFuture<'_, ()>;

    /// Return the last sequence number assigned by the mempool.
    fn last(&self) -> BoxFuture<'_, MempoolSeqNo>;

    /// Return whether accepting `additional_bytes` would exceed the configured capacity.
    fn is_near_capacity(&self, additional_bytes: u64) -> BoxFuture<'_, bool>;

    /// Return a snapshot of the active transaction count and cumulative CBOR size.
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

    fn get(&self, ids: &[TransactionId]) -> BoxFuture<'_, Vec<WithOriginalBytes<Transaction>>> {
        self.effects.external(Get::new(ids))
    }

    fn contains(&self, tx_id: &TransactionId) -> BoxFuture<'_, bool> {
        self.effects.external(ContainsTx::new(*tx_id))
    }

    fn slice(&self, from_seq: MempoolSeqNo, limit: u16) -> BoxFuture<'_, Vec<(TransactionId, u32, MempoolSeqNo)>> {
        self.effects.external(Slice::new(from_seq, limit))
    }

    fn iter(&self) -> BoxFuture<'_, Vec<WithOriginalBytes<Transaction>>> {
        self.effects.external(Iter)
    }

    fn remove(&self, ids: &[TransactionId]) -> BoxFuture<'_, ()> {
        self.effects.external(Remove::new(ids))
    }

    fn last(&self) -> BoxFuture<'_, MempoolSeqNo> {
        self.effects.external(Last)
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

    fn get(&self, ids: &[TransactionId]) -> BoxFuture<'_, Vec<WithOriginalBytes<Transaction>>> {
        let transactions = Mempool::read(self).get(ids).into_iter().cloned().collect();
        Box::pin(async move { transactions })
    }

    fn contains(&self, tx_id: &TransactionId) -> BoxFuture<'_, bool> {
        let tx_id = *tx_id;
        Box::pin(async move { Mempool::contains(self, &tx_id) })
    }

    fn slice(&self, from_seq: MempoolSeqNo, limit: u16) -> BoxFuture<'_, Vec<(TransactionId, u32, MempoolSeqNo)>> {
        Box::pin(async move { Mempool::slice(self, from_seq, limit) })
    }

    fn iter(&self) -> BoxFuture<'_, Vec<WithOriginalBytes<Transaction>>> {
        let transactions = Mempool::read(self).iter().cloned().collect();
        Box::pin(async move { transactions })
    }

    fn remove(&self, ids: &[TransactionId]) -> BoxFuture<'_, ()> {
        let tx_ids = ids.to_vec();
        Box::pin(async move { Mempool::remove(self, &tx_ids) })
    }

    fn last(&self) -> BoxFuture<'_, MempoolSeqNo> {
        Box::pin(async move { Mempool::last(self) })
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
struct Get {
    tx_ids: Vec<TransactionId>,
}

impl Get {
    pub fn new(tx_ids: &[TransactionId]) -> Self {
        Self { tx_ids: tx_ids.to_vec() }
    }
}

impl ExternalEffectAPI for Get {
    type Response = Vec<WithOriginalBytes<Transaction>>;
    // const SIMULATED_DURATION: DurationDist = effect_timings::mempool_effects__Get;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let mempool = resources.get::<ResourceMempool<Transaction>>().expect("ResourceMempool requires a mempool");
        let transactions = mempool.read().get(&self.tx_ids).into_iter().cloned().collect();
        self.wrap_sync(transactions)
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
struct Slice {
    mempool_seqno: MempoolSeqNo,
    limit: u16,
}

impl Slice {
    pub fn new(mempool_seqno: MempoolSeqNo, limit: u16) -> Self {
        Self { mempool_seqno, limit }
    }
}

impl ExternalEffectAPI for Slice {
    type Response = Vec<(TransactionId, u32, MempoolSeqNo)>;
    // const SIMULATED_DURATION: DurationDist = effect_timings::mempool_effects__Slice;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let mempool = resources.get::<ResourceMempool<Transaction>>().expect("ResourceMempool requires a mempool");
            mempool.slice(self.mempool_seqno, self.limit)
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct Iter;

impl ExternalEffectAPI for Iter {
    type Response = Vec<WithOriginalBytes<Transaction>>;
    const SIMULATED_DURATION: DurationDist = effect_timings::mempool_effects__Iter;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let mempool = resources.get::<ResourceMempool<Transaction>>().expect("ResourceMempool requires a mempool");
        let transactions = mempool.read().iter().cloned().collect();
        self.wrap_sync(transactions)
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct Remove {
    tx_ids: Vec<TransactionId>,
}

impl Remove {
    pub fn new(ids: &[TransactionId]) -> Self {
        Self { tx_ids: ids.to_vec() }
    }
}

impl ExternalEffectAPI for Remove {
    type Response = ();
    // const SIMULATED_DURATION: DurationDist = effect_timings::mempool_effects__Remove;

    #[expect(clippy::expect_used, clippy::unit_arg)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let mempool = resources.get::<ResourceMempool<Transaction>>().expect("ResourceMempool requires a mempool");
            mempool.remove(&self.tx_ids)
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct Last;

impl ExternalEffectAPI for Last {
    type Response = MempoolSeqNo;
    const SIMULATED_DURATION: DurationDist = effect_timings::mempool_effects__Last;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let mempool = resources.get::<ResourceMempool<Transaction>>().expect("ResourceMempool requires a mempool");
            mempool.last()
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
