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

use std::{fmt::Debug, marker::PhantomData};

use amaru_kernel::{Point, Transaction, TransactionId, cbor::WithOriginalBytes};
use amaru_ouroboros_traits::{Mempool, MempoolSeqNo, MempoolState, ResourceMempool, TxInsertResult, TxOrigin};
use amaru_pure_stage::{BoxFuture, DurationDist, Effects, ExternalEffectAPI, Resources, SendData, Void};
use serde::{Deserialize, Serialize};

use crate::effect_timings;

pub trait AsyncMempool<State = ()>: Send + Sync
where
    State: Send + Sync + 'static,
{
    /// Insert a transaction and its validated state fragment, preserving the origin used for relay policy.
    fn insert(
        &self,
        tx: WithOriginalBytes<Transaction>,
        state: State,
        tx_origin: TxOrigin,
    ) -> BoxFuture<'_, TxInsertResult>;

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
pub struct MempoolEffects<State = ()> {
    effects: Effects<Void>,
    state: PhantomData<fn() -> State>,
}

impl<State> MempoolEffects<State> {
    pub fn new<T: SendData + Sync + 'static>(effects: Effects<T>) -> Self {
        Self { effects: effects.erase(), state: PhantomData }
    }
}

impl<State> MempoolEffects<State>
where
    State: Clone + Send + Sync + 'static + Debug + PartialEq + Serialize + serde::de::DeserializeOwned,
{
    pub fn insert(
        &self,
        tx: WithOriginalBytes<Transaction>,
        state: State,
        tx_origin: TxOrigin,
    ) -> BoxFuture<'_, TxInsertResult> {
        self.effects.external(Insert { tx, state, tx_origin })
    }

    pub fn iter(&self) -> BoxFuture<'_, Vec<(WithOriginalBytes<Transaction>, State)>> {
        self.effects.external(Iter(PhantomData))
    }

    /// Record the tip against which the mempool has just completed revalidation.
    pub fn set_tip(&self, tip: Point) -> BoxFuture<'_, ()> {
        self.effects.external(SetTip::<State>::new(tip))
    }
}

impl<State> AsyncMempool<State> for MempoolEffects<State>
where
    State: Clone + Send + Sync + 'static + Debug + PartialEq + Serialize + serde::de::DeserializeOwned,
{
    fn insert(
        &self,
        tx: WithOriginalBytes<Transaction>,
        state: State,
        tx_origin: TxOrigin,
    ) -> BoxFuture<'_, TxInsertResult> {
        self.effects.external(Insert { tx, state, tx_origin })
    }

    fn get(&self, ids: &[TransactionId]) -> BoxFuture<'_, Vec<WithOriginalBytes<Transaction>>> {
        self.effects.external(Get::<State>::new(ids))
    }

    fn contains(&self, tx_id: &TransactionId) -> BoxFuture<'_, bool> {
        self.effects.external(ContainsTx::<State>::new(*tx_id))
    }

    fn slice(&self, from_seq: MempoolSeqNo, limit: u16) -> BoxFuture<'_, Vec<(TransactionId, u32, MempoolSeqNo)>> {
        self.effects.external(Slice::<State>::new(from_seq, limit))
    }

    fn iter(&self) -> BoxFuture<'_, Vec<WithOriginalBytes<Transaction>>> {
        self.effects.external(IterTransactions::<State>(PhantomData))
    }

    fn remove(&self, ids: &[TransactionId]) -> BoxFuture<'_, ()> {
        self.effects.external(Remove::<State>::new(ids))
    }

    fn last(&self) -> BoxFuture<'_, MempoolSeqNo> {
        self.effects.external(Last::<State>(PhantomData))
    }

    fn is_near_capacity(&self, additional_bytes: u64) -> BoxFuture<'_, bool> {
        self.effects.external(IsNearCapacity::<State> { additional_bytes, state: PhantomData })
    }

    fn state(&self) -> BoxFuture<'_, MempoolState> {
        self.effects.external(MempoolStateEffect::<State>(PhantomData))
    }
}

impl<T, State> AsyncMempool<State> for std::sync::Arc<T>
where
    T: amaru_ouroboros_traits::Mempool<WithOriginalBytes<Transaction>, State> + ?Sized,
    State: Clone + Send + Sync + 'static,
{
    fn insert(
        &self,
        tx: WithOriginalBytes<Transaction>,
        state: State,
        tx_origin: TxOrigin,
    ) -> BoxFuture<'_, TxInsertResult> {
        Box::pin(async move { Mempool::insert(self.as_ref(), tx, state, tx_origin) })
    }

    fn get(&self, ids: &[TransactionId]) -> BoxFuture<'_, Vec<WithOriginalBytes<Transaction>>> {
        let transactions = Mempool::read(self.as_ref()).get(ids).into_iter().map(|(tx, _)| tx.clone()).collect();
        Box::pin(async move { transactions })
    }

    fn contains(&self, tx_id: &TransactionId) -> BoxFuture<'_, bool> {
        let tx_id = *tx_id;
        Box::pin(async move { Mempool::contains(self.as_ref(), &tx_id) })
    }

    fn slice(&self, from_seq: MempoolSeqNo, limit: u16) -> BoxFuture<'_, Vec<(TransactionId, u32, MempoolSeqNo)>> {
        Box::pin(async move { Mempool::slice(self.as_ref(), from_seq, limit) })
    }

    fn iter(&self) -> BoxFuture<'_, Vec<WithOriginalBytes<Transaction>>> {
        let transactions = Mempool::read(self.as_ref()).iter().map(|(tx, _)| tx.clone()).collect();
        Box::pin(async move { transactions })
    }

    fn remove(&self, ids: &[TransactionId]) -> BoxFuture<'_, ()> {
        let tx_ids = ids.to_vec();
        Box::pin(async move { Mempool::remove(self.as_ref(), &tx_ids) })
    }

    fn last(&self) -> BoxFuture<'_, MempoolSeqNo> {
        Box::pin(async move { Mempool::last(self.as_ref()) })
    }

    fn is_near_capacity(&self, additional_bytes: u64) -> BoxFuture<'_, bool> {
        Box::pin(async move { Mempool::is_near_capacity(self.as_ref(), additional_bytes) })
    }

    fn state(&self) -> BoxFuture<'_, MempoolState> {
        Box::pin(async move { Mempool::state(self.as_ref()) })
    }
}

// EXTERNAL EFFECTS DEFINITIONS

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Insert<State> {
    tx: WithOriginalBytes<Transaction>,
    state: State,
    tx_origin: TxOrigin,
}

impl<State> ExternalEffectAPI for Insert<State>
where
    State: Clone + Send + Sync + 'static + Debug + PartialEq + Serialize + serde::de::DeserializeOwned,
{
    type Response = TxInsertResult;
    // const SIMULATED_DURATION: DurationDist = effect_timings::mempool_effects__Insert;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let mempool =
            resources.get::<ResourceMempool<Transaction, State>>().expect("Insert requires a mempool").clone();
        self.wrap_sync(Mempool::insert(mempool.as_ref(), self.tx.clone(), self.state.clone(), self.tx_origin.clone()))
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Iter<State>(PhantomData<State>);

impl<State> ExternalEffectAPI for Iter<State>
where
    State: Clone + Send + Sync + 'static + Debug + PartialEq + Serialize + serde::de::DeserializeOwned,
{
    type Response = Vec<(WithOriginalBytes<Transaction>, State)>;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let mempool = resources.get::<ResourceMempool<Transaction, State>>().expect("Iter requires a mempool");
        let entries = mempool.read().iter().map(|(tx, state)| (tx.clone(), state.clone())).collect();
        self.wrap_sync(entries)
    }
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Get<State> {
    tx_ids: Vec<TransactionId>,
    state: PhantomData<State>,
}

impl<State> Get<State> {
    pub fn new(tx_ids: &[TransactionId]) -> Self {
        Self { tx_ids: tx_ids.to_vec(), state: PhantomData }
    }
}

impl<State> ExternalEffectAPI for Get<State>
where
    State: Clone + Send + Sync + 'static + Debug + PartialEq + Serialize + serde::de::DeserializeOwned,
{
    type Response = Vec<WithOriginalBytes<Transaction>>;
    // const SIMULATED_DURATION: DurationDist = effect_timings::mempool_effects__Get;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let mempool = resources.get::<ResourceMempool<Transaction, State>>().expect("Get requires a mempool");
        let transactions = mempool.read().get(&self.tx_ids).into_iter().map(|(tx, _)| tx.clone()).collect();
        self.wrap_sync(transactions)
    }
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct ContainsTx<State> {
    tx_id: TransactionId,
    state: PhantomData<State>,
}

impl<State> ContainsTx<State> {
    pub fn new(tx_id: TransactionId) -> Self {
        Self { tx_id, state: PhantomData }
    }
}

impl<State> ExternalEffectAPI for ContainsTx<State>
where
    State: Clone + Send + Sync + 'static + Debug + PartialEq + Serialize + serde::de::DeserializeOwned,
{
    type Response = bool;
    // const SIMULATED_DURATION: DurationDist = effect_timings::mempool_effects__ContainsTx;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let mempool =
                resources.get::<ResourceMempool<Transaction, State>>().expect("ContainsTx requires a mempool");
            Mempool::contains(mempool.as_ref(), &self.tx_id)
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct Slice<State> {
    mempool_seqno: MempoolSeqNo,
    limit: u16,
    state: PhantomData<State>,
}

impl<State> Slice<State> {
    pub fn new(mempool_seqno: MempoolSeqNo, limit: u16) -> Self {
        Self { mempool_seqno, limit, state: PhantomData }
    }
}

impl<State> ExternalEffectAPI for Slice<State>
where
    State: Clone + Send + Sync + 'static + Debug + PartialEq + Serialize + serde::de::DeserializeOwned,
{
    type Response = Vec<(TransactionId, u32, MempoolSeqNo)>;
    // const SIMULATED_DURATION: DurationDist = effect_timings::mempool_effects__Slice;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let mempool = resources.get::<ResourceMempool<Transaction, State>>().expect("Slice requires a mempool");
            Mempool::slice(mempool.as_ref(), self.mempool_seqno, self.limit)
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct IterTransactions<State>(PhantomData<State>);

impl<State> ExternalEffectAPI for IterTransactions<State>
where
    State: Clone + Send + Sync + 'static + Debug + PartialEq + Serialize + serde::de::DeserializeOwned,
{
    type Response = Vec<WithOriginalBytes<Transaction>>;
    const SIMULATED_DURATION: DurationDist = effect_timings::mempool_effects__Iter;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        let mempool = resources.get::<ResourceMempool<Transaction, State>>().expect("Iter requires a mempool");
        let transactions = mempool.read().iter().map(|(tx, _)| tx.clone()).collect();
        self.wrap_sync(transactions)
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct Remove<State> {
    tx_ids: Vec<TransactionId>,
    state: PhantomData<State>,
}

impl<State> Remove<State> {
    pub fn new(ids: &[TransactionId]) -> Self {
        Self { tx_ids: ids.to_vec(), state: PhantomData }
    }
}

impl<State> ExternalEffectAPI for Remove<State>
where
    State: Clone + Send + Sync + 'static + Debug + PartialEq + Serialize + serde::de::DeserializeOwned,
{
    type Response = ();
    // const SIMULATED_DURATION: DurationDist = effect_timings::mempool_effects__Remove;

    #[expect(clippy::expect_used, clippy::unit_arg)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let mempool = resources.get::<ResourceMempool<Transaction, State>>().expect("Remove requires a mempool");
            Mempool::remove(mempool.as_ref(), &self.tx_ids)
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct Last<State>(PhantomData<State>);

impl<State> ExternalEffectAPI for Last<State>
where
    State: Clone + Send + Sync + 'static + Debug + PartialEq + Serialize + serde::de::DeserializeOwned,
{
    type Response = MempoolSeqNo;
    const SIMULATED_DURATION: DurationDist = effect_timings::mempool_effects__Last;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let mempool = resources.get::<ResourceMempool<Transaction, State>>().expect("Last requires a mempool");
            Mempool::last(mempool.as_ref())
        })
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct IsNearCapacity<State> {
    additional_bytes: u64,
    state: PhantomData<State>,
}

impl<State> ExternalEffectAPI for IsNearCapacity<State>
where
    State: Clone + Send + Sync + 'static + Debug + PartialEq + Serialize + serde::de::DeserializeOwned,
{
    type Response = bool;
    // const SIMULATED_DURATION: DurationDist = effect_timings::mempool_effects__IsNearCapacity;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let mempool =
                resources.get::<ResourceMempool<Transaction, State>>().expect("IsNearCapacity requires a mempool");
            Mempool::is_near_capacity(mempool.as_ref(), self.additional_bytes)
        })
    }
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct MempoolStateEffect<State>(PhantomData<State>);

impl<State> ExternalEffectAPI for MempoolStateEffect<State>
where
    State: Clone + Send + Sync + 'static + Debug + PartialEq + Serialize + serde::de::DeserializeOwned,
{
    type Response = MempoolState;
    const SIMULATED_DURATION: DurationDist = effect_timings::mempool_effects__State;

    #[expect(clippy::expect_used)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let mempool = resources.get::<ResourceMempool<Transaction, State>>().expect("State requires a mempool");
            Mempool::state(mempool.as_ref())
        })
    }
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct SetTip<State> {
    tip: Point,
    state: PhantomData<State>,
}

impl<State> SetTip<State> {
    fn new(tip: Point) -> Self {
        Self { tip, state: PhantomData }
    }
}

impl<State> ExternalEffectAPI for SetTip<State>
where
    State: Clone + Send + Sync + 'static + Debug + PartialEq + Serialize + serde::de::DeserializeOwned,
{
    type Response = ();

    #[expect(clippy::expect_used, clippy::unit_arg)]
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync({
            let mempool = resources.get::<ResourceMempool<Transaction, State>>().expect("SetTip requires a mempool");
            Mempool::set_tip(mempool.as_ref(), self.tip)
        })
    }
}
