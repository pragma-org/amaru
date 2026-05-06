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
use amaru_ouroboros::{MempoolError, MempoolMsg, MempoolSeqNo, TxId, TxInsertResult, TxOrigin, TxRejectReason};
use amaru_protocols::mempool_effects::MemoryPool;
use pure_stage::{Effects, StageRef, Void};

use crate::{
    effects::{Ledger, LedgerOps, Metrics},
    stages::mempool::traces::{RevalidationOutcome, emit_tx_received, record_insert, record_revalidation},
};

/// Create a stage that accepts messages to validate then insert transactions into the mempool.
/// The mempool messages contain a caller reference that is used to return insertion results.
///
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
            let tx_id = TxId::from(&tx);
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
                let tx_id = TxId::from(&tx);
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
            }
            Err(e) => {
                tracing::error!(%e, "failed to apply new tip to the mempool");
                eff.terminate::<Void>().await;
            }
        },
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
        Err(error) => Ok(TxInsertResult::rejected(TxId::from(&tx), TxRejectReason::Invalid(error))),
    }
}

/// Revalidate all the mempool transactions when a new tip has been adopted
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
            invalid_tx_ids.push(TxId::from(&tx));
        }
    }

    if !invalid_tx_ids.is_empty() {
        memory_pool.remove_txs(&invalid_tx_ids).await?;
    }

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

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MempoolStageState {
    waiters: Vec<MempoolWaiter>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct MempoolWaiter {
    seq_no: MempoolSeqNo,
    caller: StageRef<()>,
}
