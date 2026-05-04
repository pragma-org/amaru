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

#[cfg(not(target_arch = "wasm32"))]
use std::sync::OnceLock;

#[cfg(not(target_arch = "wasm32"))]
use opentelemetry::KeyValue;

#[cfg(not(target_arch = "wasm32"))]
use crate::{Counter, Gauge};
use crate::{Meter, MetricRecorder, MetricsEvent};

// EVENTS

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TxInsertionOrigin {
    Local,
    Remote,
}

impl TxInsertionOrigin {
    pub fn as_label(self) -> &'static str {
        match self {
            TxInsertionOrigin::Local => "local",
            TxInsertionOrigin::Remote => "remote",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TxInsertionResult {
    Accepted,
    RejectedInvalid,
    RejectedDuplicate,
    RejectedFull,
}

impl TxInsertionResult {
    pub fn as_label(self) -> &'static str {
        match self {
            TxInsertionResult::Accepted => "accepted",
            TxInsertionResult::RejectedInvalid => "rejected_invalid",
            TxInsertionResult::RejectedDuplicate => "rejected_duplicate",
            TxInsertionResult::RejectedFull => "rejected_full",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TxEvictedReason {
    InvalidAfterTip,
}

impl TxEvictedReason {
    pub fn as_label(self) -> &'static str {
        match self {
            TxEvictedReason::InvalidAfterTip => "invalid_after_tip",
        }
    }
}

/// Mempool metric events for the OpenTelemetry:
///
///  - TxInsertion is emitted for each insertion attempt.
///  - TxEvicted is emitted when transactions are removed from the mempool after a revalidation.
///  - Revalidated marks a revalidation of the mempool when there's a new tip and records its duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MempoolMetricEvent {
    TxInsertion { origin: TxInsertionOrigin, result: TxInsertionResult },
    TxEvicted { reason: TxEvictedReason, count: u64 },
    Revalidated { duration_micros: u64 },
}

// METRICS

/// Mempool metrics aggregate a mempool metric event + the mempool state at the time of the event
/// emission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MempoolMetrics {
    pub size_bytes: u64,
    pub tx_count: u64,
    pub event: MempoolMetricEvent,
}

#[cfg(target_arch = "wasm32")]
impl MetricRecorder for MempoolMetrics {
    fn record_to_meter(&self, _meter: &Meter) {
        // no-op in wasm32 environment, because of `opentelemetry` dependency on `js-sys`
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl MetricRecorder for MempoolMetrics {
    fn record_to_meter(&self, meter: &Meter) {
        static SIZE_BYTES: OnceLock<Gauge<u64>> = OnceLock::new();
        static TX_COUNT: OnceLock<Gauge<u64>> = OnceLock::new();
        static TXS_PROCESSED: OnceLock<Counter<u64>> = OnceLock::new();
        static TXS_SYNC_DURATION: OnceLock<Gauge<u64>> = OnceLock::new();
        static TXS_SYNC_DURATION_TOTAL: OnceLock<Counter<u64>> = OnceLock::new();
        static TX_INSERTIONS: OnceLock<Counter<u64>> = OnceLock::new();

        // Metrics common to amaru and cardano-node

        let size_bytes = SIZE_BYTES.get_or_init(|| {
            meter
                .u64_gauge("cardano_node_metrics_mempoolBytes_int")
                .with_description("current cumulative CBOR size of transactions held in the mempool")
                .with_unit("int")
                .build()
        });

        let tx_count = TX_COUNT.get_or_init(|| {
            meter
                .u64_gauge("cardano_node_metrics_txsInMempool_int")
                .with_description("current number of transactions held in the mempool")
                .with_unit("int")
                .build()
        });

        let txs_processed = TXS_PROCESSED.get_or_init(|| {
            meter
                .u64_counter("cardano_node_metrics_txsProcessedNum_int")
                .with_description("total transactions moved out of the mempool after revalidation")
                .with_unit("int")
                .build()
        });

        let txs_sync_duration = TXS_SYNC_DURATION.get_or_init(|| {
            meter
                .u64_gauge("cardano_node_metrics_txsSyncDuration_int")
                .with_description("latest time to sync the mempool in ms after block adoption")
                .with_unit("int")
                .build()
        });

        // cumulative mempool sync duration.
        let txs_sync_duration_total = TXS_SYNC_DURATION_TOTAL.get_or_init(|| {
            meter
                .u64_counter("cardano_node_metrics_txsSyncDurationTotal_int")
                .with_description("cumulative time spent syncing the mempool in ms after block adoption")
                .with_unit("int")
                .build()
        });

        // Amaru-specific metrics

        let tx_insertions = TX_INSERTIONS.get_or_init(|| {
            meter
                .u64_counter("cardano_node_metrics_mempoolTxInsertionsNum_int")
                .with_description("transaction insertion attempts, labelled by origin and result")
                .with_unit("int")
                .build()
        });

        size_bytes.record(self.size_bytes, &[]);
        tx_count.record(self.tx_count, &[]);

        match self.event {
            MempoolMetricEvent::TxInsertion { origin, result } => {
                let attributes =
                    &[KeyValue::new("origin", origin.as_label()), KeyValue::new("result", result.as_label())];
                tx_insertions.add(1, attributes);
            }
            MempoolMetricEvent::TxEvicted { reason: _, count } => {
                if count > 0 {
                    txs_processed.add(count, &[]);
                }
            }
            MempoolMetricEvent::Revalidated { duration_micros } => {
                let duration_ms = (duration_micros + 500) / 1_000;
                txs_sync_duration.record(duration_ms, &[]);
                txs_sync_duration_total.add(duration_ms, &[]);
            }
        }
    }
}

impl From<MempoolMetrics> for MetricsEvent {
    fn from(value: MempoolMetrics) -> Self {
        MetricsEvent::MempoolMetrics(value)
    }
}
