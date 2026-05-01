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

use std::time::Instant;

use amaru_kernel::{Tip, Transaction};
use amaru_ouroboros::{MempoolError, MempoolMsg, MempoolSeqNo, TxInsertResult, TxOrigin, TxRejectReason};
use amaru_protocols::mempool_effects::MemoryPool;
use pure_stage::{Effects, StageRef, Void};

use crate::{
    effects::{Ledger, LedgerOps, Metrics},
    stages::mempool::traces::{RevalidationOutcome, emit_tx_received, record_insert, record_revalidation},
};

/// The Mempool stage is a pure_stage actor that coordinates validation and insertion of
/// transactions into the shared mempool via effects, while managing asynchronous waiter
/// notifications for sequence number readiness and mempool capacity events.
///
/// It accepts `MempoolMsg` inputs (re-exported from `amaru_ouroboros`):
/// - `Insert { tx, origin, caller }` / `InsertBatch { txs, origin, caller }`: validate each
///   tx via the `Ledger` effect (`validate_tx`); on success call `MemoryPool::insert`,
///   on ledger failure construct `TxInsertResult::rejected(..., Invalid(...))`. Successful
///   `Accepted { seq_no, .. }` results trigger `notify_ready_waiters`. Always reply to the
///   embedded `caller: StageRef` with `Ok(TxInsertResult)` or `Ok(Vec<TxInsertResult>)` (or
///   `Err(MempoolInsertError { tx_id, error })` on hard `MempoolError` from insert). Batch
///   aborts on first hard error after replying Err.
/// - `WaitForAtLeast { seq_no, caller }`: immediately `send(())` if `memory_pool.last_seq_no() >= seq_no`,
///   else queue `MempoolWaiter` in state for later notification on an accepting insert.
/// - `NewTip(tip)`: revalidate all current mempool txs against ledger via `apply_new_tip`,
///   remove invalid ones (side-effect), notify capacity waiters if any were removed.
/// - `SubscribeCapacity { caller }`: register `caller: StageRef<()>` for one-shot notification
///   on future capacity-relieving NewTip.
///
/// The stage maintains only coordination state (`MempoolStageState`: lists of seq-waiters and
/// capacity subscribers; serializable). All persistent state and heavy ops live in the
/// `MemoryPool` and `Ledger` effects (backed by resources like `InMemoryMempool` and tx validators).
/// No child stages are spawned; work is performed directly in the handler (with awaits on effects).
/// Results flow back to callers (e.g. tx_submission manager, adopt_chain logic) exclusively via
/// the `StageRef` callbacks in the messages using `Effects::send`. Side effects are strictly
/// limited to the two effects; the stage itself is stateless w.r.t. tx contents.
///
/// This stage is initialized in the consensus graph (via wiring with default state) and
/// receives `NewTip` updates on chain adoption (for revalidation/clearing) and insert requests
/// for tx submission.
///
/// ## FIXME: Suspected Issues (isolated analysis)
/// - **Inconsistent batch error handling + partial side effects**: On hard error in `InsertBatch`, `eff.send(Err(...))` + immediate `return state;`. Prior txs in the same batch already performed `memory_pool.insert` (and seq-waiter notifies if accepted). Caller sees only an error (for the failing `tx_id`), not a partial `Vec` of results. Single-`Insert` path always completes a send. Risk of callers observing inconsistent committed state vs. reply.
/// - **Test coverage gaps for variants + error paths + deserializer**: Only `InsertBatch` + `NewTip` exercised (with `TxOrigin::Local`, mock rejections leading to in-result `rejected`, and duplicates). No coverage of single `Insert`, `WaitForAtLeast`, `SubscribeCapacity`, or paths that surface top-level `MempoolInsertError` (vs. `TxInsertResult::rejected`). Guard registers only the `Vec<Result<...>>` form; scalar `Insert` reply type is untested and could cause simulation/serde issues.
/// - **Silent drop on remove failure during reval**: `apply_new_tip` on `memory_pool.remove_txs` error: `tracing::error!` + `return 0` (no capacity notify). Invalid txs may remain in mempool after `NewTip`. Potential for stuck invalid entries post-chain switch.
/// - **Capacity notifications narrowly scoped + one-shot**: `notify_capacity_waiters` (which drains) *only* on `NewTip` with `removed > 0`. Successful inserts never notify. Docstring notes "Subscribers that still need to be notified after re-evaluating must re-subscribe." Subscribers can easily miss events or leak registrations.
/// - **No child stages / potential HOL blocking**: Validation, per-tx awaits on effects, full-mempool iteration in `apply_new_tip`, and all notifies happen directly in the handler. No `eff.spawn` for heavy work. Large mempools or slow ledger effects on `NewTip`/batches can block other messages (Insert/Wait/etc.) to this stage.
/// - **Unbounded waiter accumulation + no cleanup**: `waiters` and `capacity_waiters` `Vec`s grow with no size limits, timeouts, or GC. `MempoolWaiter` holds `StageRef`s. Serializable state means persistence, but restarts lose pending notifications. Risk of unbounded growth or "stuck" waiters.
/// - **Reply asymmetry and fire-and-forget risks**: Insert*/Wait guarantee one reply. `NewTip`/`Subscribe` are pure side-effect or registration (no direct reply). Waiters rely on later `eff.send(())`; any downstream or effect failure leaves callers hanging with no error surfacing in this stage.
/// - **Full O(n) revalidation on every NewTip**: `mempool_txs().await` + sequential per-tx `validate_tx` + conditional remove. No sequencing, batching, or incremental logic. Cost scales with mempool size on every tip adoption.
/// - **Batch aborts remaining work on first hard error**: Loop processes sequentially but `return`s on first `Err` from `validate_and_insert`. No attempt of later txs; caller gets error for the failure point only.
/// - **Design notes / minor**: Ledger validation failures never produce `MempoolError` (always mapped to `rejected` result — intentional for submission UX). The `return state;` in batch error is functionally equivalent to fallthrough but skips any hypothetical post-match logic. Per-accepted notifies inside batch loop are fine (drain logic handles it).
pub async fn stage(state: MempoolStageState, msg: MempoolMsg, eff: Effects<MempoolMsg>) -> MempoolStageState {
    let memory_pool = MemoryPool::new(eff.clone());
    let ledger = Ledger::new(eff.clone());
    let metrics_ops = Metrics::new(&eff);
    let mut state = state;
    match msg {
        MempoolMsg::WaitForAtLeast { seq_no, caller } => {
            if memory_pool.last_seq_no().await >= seq_no {
                eff.send(&caller, ()).await;
            } else {
                state.waiters.push(MempoolWaiter { seq_no, caller });
            }
        }
        MempoolMsg::Insert { tx, origin, caller } => {
            let tx = *tx;
            let tx_id = tx.tx_id();
            emit_tx_received(&tx_id, &origin);

            match validate_and_insert(&ledger, &memory_pool, tx, &origin).await {
                Ok(result) => {
                    record_insert(memory_pool.state().await, &metrics_ops, &origin, &result).await;
                    match result {
                        TxInsertResult::Accepted { seq_no, .. } => {
                            notify_ready_waiters(&mut state, &eff, seq_no).await;
                        }
                        TxInsertResult::Rejected { tx_id, ref reason } => {
                            tracing::info!(%tx_id, %reason, "transaction rejected by mempool");
                        }
                    }
                    eff.send(&caller, Ok(result)).await;
                }
                Err(e) => {
                    tracing::error!(%e, %tx_id, "cannot insert transaction into the mempool");
                    eff.send(&caller, Err(e)).await;
                }
            };
        }
        MempoolMsg::InsertBatch { txs, origin, caller } => {
            let mut results = Vec::with_capacity(txs.len());
            for tx in txs {
                let tx_id = tx.tx_id();
                emit_tx_received(&tx_id, &origin);
                match validate_and_insert(&ledger, &memory_pool, tx, &origin).await {
                    Ok(result) => {
                        record_insert(memory_pool.state().await, &metrics_ops, &origin, &result).await;
                        match result {
                            TxInsertResult::Accepted { seq_no, .. } => {
                                notify_ready_waiters(&mut state, &eff, seq_no).await;
                            }
                            TxInsertResult::Rejected { tx_id, ref reason } => {
                                tracing::info!(%tx_id, %reason, "transaction rejected by mempool");
                            }
                        }
                        results.push(result);
                    }
                    Err(e) => {
                        tracing::error!(%e, %tx_id, "cannot insert transaction into the mempool");
                        eff.send(&caller, Err(e)).await;
                        return state;
                    }
                }
            }
            eff.send(&caller, Ok(results)).await;
        }
        MempoolMsg::NewTip(tip) => match apply_new_tip(&ledger, &memory_pool, tip).await {
            Ok(outcome) => {
                record_revalidation(memory_pool.state().await, &metrics_ops, &outcome).await;
                if !outcome.evicted_tx_ids.is_empty() {
                    notify_capacity_waiters(&mut state, &eff).await;
                }
            }
            Err(e) => {
                tracing::error!(%e, "failed to apply new tip to the mempool");
                eff.terminate::<Void>().await;
            }
        },
        MempoolMsg::SubscribeCapacity { caller } => {
            state.capacity_waiters.push(caller);
        }
    }
    state
}

/// Validate a transaction against the current ledger state
/// and insert it into the mempool if it is valid.
///
async fn validate_and_insert(
    ledger: &Ledger,
    memory_pool: &MemoryPool,
    tx: Transaction,
    origin: &TxOrigin,
) -> Result<TxInsertResult, MempoolError> {
    match ledger.validate_tx(&tx).await {
        Ok(()) => memory_pool.insert(tx, origin.clone()).await,
        Err(error) => Ok(TxInsertResult::rejected(tx.tx_id(), TxRejectReason::Invalid(error))),
    }
}

/// Revalidate all the mempool transactions when a new tip has been adopted.
async fn apply_new_tip(
    ledger: &Ledger,
    memory_pool: &MemoryPool,
    tip: Tip,
) -> Result<RevalidationOutcome, MempoolError> {
    let started = Instant::now();
    let txs = memory_pool.mempool_txs().await;
    let total_before = txs.len() as u64;

    let mut invalid_tx_ids = vec![];
    for tx in txs {
        if ledger.validate_tx(&tx).await.is_err() {
            invalid_tx_ids.push(tx.tx_id());
        }
    }

    if !invalid_tx_ids.is_empty() {
        memory_pool.remove_txs(&invalid_tx_ids).await?;
    }

    tracing::debug!(%tip, invalidated_txs = invalid_tx_ids.len(), "revalidated mempool after new tip");
    Ok(RevalidationOutcome {
        tip_slot: u64::from(tip.slot()),
        total_before,
        evicted_tx_ids: invalid_tx_ids,
        duration_micros: started.elapsed().as_micros() as u64,
    })
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
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct MempoolWaiter {
    seq_no: MempoolSeqNo,
    caller: StageRef<()>,
}
