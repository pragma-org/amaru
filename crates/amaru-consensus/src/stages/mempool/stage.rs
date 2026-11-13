// Copyright 2026 PRAGMA
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
    collections::{BTreeMap, VecDeque},
    time::Instant,
};

use amaru_kernel::{Hash, Hasher, Point, Transaction, cbor::WithOriginalBytes};
use amaru_ouroboros::{MempoolMsg, MempoolSeqNo, TxInsertResult, TxOrigin, TxRejectReason};
use amaru_protocols::{
    mempool_effects::{AsyncMempool, MempoolEffects},
    store_effects::Store,
};
use amaru_pure_stage::{Effects, StageRef};

use crate::{
    effects::{Ledger, LedgerOps, Metrics},
    stages::mempool::traces::{RevalidationOutcome, emit_tx_received, record_insert, record_revalidation},
};

/// The Mempool stage is a amaru_pure_stage actor that coordinates validation and insertion of
/// transactions into the shared mempool via effects, while managing asynchronous waiter
/// notifications for sequence number readiness and mempool capacity events.
///
/// It accepts `MempoolMsg` messages:
/// - `Insert { tx, origin, caller }` / `InsertBatch { txs, origin, caller }`: validate each
///   tx via the `Ledger` effect (`validate_tx`);
///     - On success the transaction(s) are accepted into the mempool (via the mempool effect),
///     - On failure a rejection result (`TxInsertResult::rejected(..., Invalid(...))`) is returned.
///     - When a transaction is accepted into the mempool, the waiters are notified (via `notify_ready_waiters`)
///       in order to transmit available transactions upstream.
///
///   The `caller` gets a `TxInsertResult` reply (or a `Vec<TxInsertResult>` in case of a batch insertion).
/// - `WaitForAtLeast { seq_no, caller }`: wait until `memory_pool.last() >= seq_no`:
///     - If this is the case when the message is received, the caller gets a reply right away,
///     - Otherwise, it is queued as a `MempoolWaiter` to get a notification as soon as a transaction is
///       inserted into the mempool.
///
/// - `NewTip(tip)`: revalidate all current mempool txs against ledger via `apply_new_tip`:
///     - Remove invalid ones (via `Mempool::remove_tx`)
///     - Notify capacity waiters if any were removed
///
///   TODO: optimize this call; it is not necessary to revalidate all the transactions. We can,
///   in principle build a dependency graph of all the transactions in the mempool and check, level by level,
///   which parts of the graph are still valid based on the new ledger state.
///
/// - `SubscribeCapacity { caller }`: register `caller` for a one-shot notification
///   when `NewTip` frees some pool capacity.
///
pub async fn stage(state: MempoolStageState, msg: MempoolMsg, eff: Effects<MempoolMsg>) -> MempoolStageState {
    let mempool = MempoolEffects::new(eff.clone());
    let ledger = Ledger::new(eff.clone());
    let metrics_ops = Metrics::new(&eff);
    let mut state = state;
    match msg {
        MempoolMsg::WaitForAtLeast { seq_no, caller } => {
            if mempool.last().await >= seq_no {
                eff.send(&caller, ()).await;
            } else {
                state.waiters.push(MempoolWaiter { seq_no, caller });
            }
        }
        MempoolMsg::Insert { tx, origin, caller } => {
            let tx = *tx;
            let tx_id = tx.tx_id();
            emit_tx_received(&tx_id, &origin);

            let result = validate_and_insert(&ledger, &mempool, &mut state.invalid_txs_cache, tx, &origin).await;
            record_insert(mempool.state().await, &metrics_ops, &origin, &result).await;
            if let TxInsertResult::Accepted { seq_no, .. } = result {
                notify_ready_waiters(&mut state, &eff, seq_no).await;
            }
            eff.send(&caller, result).await;
        }
        MempoolMsg::InsertBatch { txs, origin, caller } => {
            let mut results = Vec::with_capacity(txs.len());
            for tx in txs {
                let tx_id = tx.tx_id();
                emit_tx_received(&tx_id, &origin);
                let result = validate_and_insert(&ledger, &mempool, &mut state.invalid_txs_cache, tx, &origin).await;
                record_insert(mempool.state().await, &metrics_ops, &origin, &result).await;
                if let TxInsertResult::Accepted { seq_no, .. } = result {
                    notify_ready_waiters(&mut state, &eff, seq_no).await;
                }
                results.push(result);
            }
            eff.send(&caller, results).await;
        }
        MempoolMsg::NewTip(tip) => {
            let store = Store::new(eff.clone());
            let outcome = apply_new_tip(&ledger, &mempool, tip).await;
            mempool.set_tip(tip).await;
            state.invalid_txs_cache.clear();
            record_revalidation(mempool.state().await, &metrics_ops, &store, tip, &outcome).await;
            if !outcome.evicted_tx_ids.is_empty() {
                notify_capacity_waiters(&mut state, &eff).await;
            }
        }
        MempoolMsg::SubscribeCapacity { caller } => {
            state.capacity_waiters.push(caller);
        }
    }
    state
}

/// Validate a transaction against the current ledger state and insert it into the mempool if it is valid.
///
/// `invalid_txs_cache` is a cache that is used to avoid re-validating invalid transactions.
async fn validate_and_insert(
    ledger: &Ledger,
    mempool: &MempoolEffects,
    invalid_txs_cache: &mut InvalidTxsCache,
    tx: WithOriginalBytes<Transaction>,
    origin: &TxOrigin,
) -> TxInsertResult {
    let digest = tx_digest(&tx);
    if let Some(result) = invalid_txs_cache.get(&digest) {
        return result.clone();
    }
    match ledger.validate_tx(&tx).await {
        Ok(()) => mempool.insert(tx, origin.clone()).await,
        Err(error) => {
            let result = TxInsertResult::rejected(tx.tx_id(), TxRejectReason::Invalid(error));
            invalid_txs_cache.remember(digest, &result);
            result
        }
    }
}

/// Revalidate all the mempool transactions when a new tip has been adopted.
async fn apply_new_tip(ledger: &Ledger, mempool: &MempoolEffects, tip: Point) -> RevalidationOutcome {
    let started = Instant::now();
    let txs = mempool.iter().await;
    let total_before = txs.len() as u64;

    let mut invalid_tx_ids = vec![];
    for tx in txs {
        if ledger.validate_tx(&tx).await.is_err() {
            invalid_tx_ids.push(tx.tx_id());
        }
    }

    if !invalid_tx_ids.is_empty() {
        mempool.remove(&invalid_tx_ids).await;
    }

    RevalidationOutcome {
        tip_slot: tip.slot(),
        total_before,
        evicted_tx_ids: invalid_tx_ids,
        duration_micros: started.elapsed().as_micros() as u64,
    }
}

/// Notify the waiters whose target sequence number has just been reached.
async fn notify_ready_waiters(state: &mut MempoolStageState, eff: &Effects<MempoolMsg>, reached_seq_no: MempoolSeqNo) {
    if state.waiters.is_empty() {
        return;
    }

    let mut ready_waiters = Vec::new();
    let mut pending_waiters = Vec::with_capacity(state.waiters.len());

    for waiter in state.waiters.drain(..) {
        if waiter.seq_no <= reached_seq_no {
            ready_waiters.push(waiter.caller);
        } else {
            pending_waiters.push(waiter);
        }
    }

    state.waiters = pending_waiters;

    for caller in ready_waiters {
        eff.send(&caller, ()).await;
    }
}

/// Notify all one-shot capacity subscribers and drain the list. Subscribers that still need
/// to be notified after re-evaluating must re-subscribe.
async fn notify_capacity_waiters(state: &mut MempoolStageState, eff: &Effects<MempoolMsg>) {
    for caller in state.capacity_waiters.drain(..) {
        eff.send(&caller, ()).await;
    }
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MempoolStageState {
    waiters: Vec<MempoolWaiter>,
    capacity_waiters: Vec<StageRef<()>>,
    invalid_txs_cache: InvalidTxsCache,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct MempoolWaiter {
    seq_no: MempoolSeqNo,
    caller: StageRef<()>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub(super) struct InvalidTxsCache {
    results: BTreeMap<TxDigest, TxInsertResult>,
    /// Digests in the order in which their results were remembered, oldest first.
    order: VecDeque<TxDigest>,
}

impl InvalidTxsCache {
    /// Between two tips, which are a few tens of seconds apart, a node sees far fewer distinct invalid
    /// transactions than this. The bound only limits the memory use if a peer floods the node.
    pub(super) const CAPACITY: usize = 256;

    pub(super) fn get(&self, digest: &TxDigest) -> Option<&TxInsertResult> {
        self.results.get(digest)
    }

    pub(super) fn remember(&mut self, digest: TxDigest, result: &TxInsertResult) {
        if self.results.insert(digest, result.clone()).is_some() {
            return;
        }
        self.order.push_back(digest);
        if self.order.len() > Self::CAPACITY
            && let Some(oldest) = self.order.pop_front()
        {
            self.results.remove(&oldest);
        }
    }

    fn clear(&mut self) {
        self.results.clear();
        self.order.clear();
    }

    #[cfg(test)]
    pub(super) fn of(results: impl IntoIterator<Item = (TxDigest, TxInsertResult)>) -> Self {
        let mut invalid_txs_cache = Self::default();
        for (digest, result) in results {
            invalid_txs_cache.remember(digest, &result);
        }
        invalid_txs_cache
    }
}

/// The hash of a complete transaction as received: body, witnesses and auxiliary data.
///
/// A `TransactionId` only identifies the body, but validation also checks the witnesses. Two
/// transactions with the same body and different witnesses can have different validation results.
pub(super) type TxDigest = Hash<32>;

pub(super) fn tx_digest(tx: &WithOriginalBytes<Transaction>) -> TxDigest {
    Hasher::<256>::hash(tx.as_bytes())
}

#[cfg(test)]
impl MempoolStageState {
    pub(super) fn with_invalid_txs_cache(invalid_txs_cache: InvalidTxsCache) -> Self {
        Self { invalid_txs_cache, ..Self::default() }
    }
}
