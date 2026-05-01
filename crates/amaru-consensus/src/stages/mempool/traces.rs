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

use amaru_metrics::mempool::{
    MempoolMetricEvent, MempoolMetrics, TxEvictedReason, TxInsertionOrigin, TxInsertionResult,
};
use amaru_observability::trace_record;
use amaru_ouroboros::MempoolMsg;
use amaru_ouroboros_traits::{MempoolState, TxId, TxInsertResult, TxOrigin, TxRejectReason};

use crate::effects::{Metrics, MetricsOps};

/// Add traces for a transaction that is candidate for mempool insertion.
pub(super) fn emit_tx_received(tx_id: &TxId, origin: &TxOrigin) {
    trace_record!(
        amaru_observability::amaru::mempool::TX_RECEIVED,
        tx_id = tx_id.to_string(),
        origin = tx_origin_label(origin)
    );
    if let TxOrigin::Remote(peer) = origin {
        trace_record!(
            amaru_observability::amaru::mempool::TX_RECEIVED_DETAIL,
            tx_id = tx_id.to_string(),
            peer = peer.to_string()
        );
    }
}

/// Add a trace to register the result of a mempool insertion (successful or not).
/// Emit the corresponding metric.
pub(super) async fn record_insert(
    mempool_state: MempoolState,
    metrics: &Metrics<'_, MempoolMsg>,
    origin: &TxOrigin,
    result: &TxInsertResult,
) {
    match result {
        TxInsertResult::Accepted { tx_id, seq_no } => {
            trace_record!(
                amaru_observability::amaru::mempool::TX_ACCEPTED,
                tx_id = tx_id.to_string(),
                seq_no = seq_no.0,
                origin = tx_origin_label(origin)
            );
        }
        TxInsertResult::Rejected { tx_id, reason } => {
            let reason_label = reject_reason_label(reason);
            match reason {
                TxRejectReason::Invalid(err) => {
                    let validation_error = err.to_string();
                    trace_record!(
                        amaru_observability::amaru::mempool::TX_REJECTED,
                        tx_id = tx_id.to_string(),
                        reason = reason_label,
                        validation_error = validation_error
                    );
                }
                TxRejectReason::Duplicate | TxRejectReason::MempoolFull => {
                    trace_record!(
                        amaru_observability::amaru::mempool::TX_REJECTED,
                        tx_id = tx_id.to_string(),
                        reason = reason_label
                    );
                }
            }
        }
    }
    emit_metrics(
        mempool_state,
        metrics,
        MempoolMetricEvent::TxInsertion { origin: tx_origin_metric(origin), result: insertion_result_metric(result) },
    )
    .await;
}

/// When a new tip is adopted, add a trace recording the result of the transactions revalidations,
/// and emit metrics
pub(super) async fn record_revalidation(
    mempool_state: MempoolState,
    metrics: &Metrics<'_, MempoolMsg>,
    outcome: &RevalidationOutcome,
) {
    let evicted_count = outcome.evicted_tx_ids.len() as u64;

    for tx_id in &outcome.evicted_tx_ids {
        trace_record!(
            amaru_observability::amaru::mempool::TX_EVICTED,
            tx_id = tx_id.to_string(),
            reason = "invalid_after_tip".to_string()
        );
    }
    if evicted_count > 0 {
        emit_metrics(
            mempool_state,
            metrics,
            MempoolMetricEvent::TxEvicted { reason: TxEvictedReason::InvalidAfterTip, count: evicted_count },
        )
        .await;
    }

    trace_record!(
        amaru_observability::amaru::mempool::REVALIDATION_DETAIL,
        tip_slot = outcome.tip_slot,
        total_before = outcome.total_before,
        evicted_count = evicted_count,
        duration_micros = outcome.duration_micros
    );

    emit_metrics(mempool_state, metrics, MempoolMetricEvent::Revalidated).await;
}

pub(super) struct RevalidationOutcome {
    pub(super) tip_slot: u64,
    pub(super) total_before: u64,
    pub(super) evicted_tx_ids: Vec<TxId>,
    pub(super) duration_micros: u64,
}

async fn emit_metrics(mempool_state: MempoolState, metrics: &Metrics<'_, MempoolMsg>, event: MempoolMetricEvent) {
    let MempoolState { size_bytes, tx_count } = mempool_state;
    metrics.record(MempoolMetrics { size_bytes, tx_count, event }.into()).await;
}

fn tx_origin_label(origin: &TxOrigin) -> String {
    match origin {
        TxOrigin::Local => "local",
        TxOrigin::Remote(_) => "remote",
    }
    .to_string()
}

fn tx_origin_metric(origin: &TxOrigin) -> TxInsertionOrigin {
    match origin {
        TxOrigin::Local => TxInsertionOrigin::Local,
        TxOrigin::Remote(_) => TxInsertionOrigin::Remote,
    }
}

fn reject_reason_label(reason: &TxRejectReason) -> String {
    match reason {
        TxRejectReason::Invalid(_) => "invalid",
        TxRejectReason::Duplicate => "duplicate",
        TxRejectReason::MempoolFull => "mempool_full",
    }
    .to_string()
}

fn insertion_result_metric(result: &TxInsertResult) -> TxInsertionResult {
    match result {
        TxInsertResult::Accepted { .. } => TxInsertionResult::Accepted,
        TxInsertResult::Rejected { reason, .. } => match reason {
            TxRejectReason::Invalid(_) => TxInsertionResult::RejectedInvalid,
            TxRejectReason::Duplicate => TxInsertionResult::RejectedDuplicate,
            TxRejectReason::MempoolFull => TxInsertionResult::RejectedFull,
        },
    }
}
