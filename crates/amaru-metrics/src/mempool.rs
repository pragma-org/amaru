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

/// One mempool event for the OpenTelemetry meter. Each emission refreshes
/// the gauges to the supplied snapshot and increments the counter matching
/// the variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MempoolMetricEvent {
    TxInsertion { origin: TxInsertionOrigin, result: TxInsertionResult },
    TxEvicted { reason: TxEvictedReason },
    Revalidated,
}

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
        static TX_INSERTIONS: OnceLock<Counter<u64>> = OnceLock::new();
        static TX_EVICTED: OnceLock<Counter<u64>> = OnceLock::new();
        static REVALIDATIONS: OnceLock<Counter<u64>> = OnceLock::new();

        let size_bytes = SIZE_BYTES.get_or_init(|| {
            meter
                .u64_gauge("cardano_node_mempool_size_bytes")
                .with_description("current cumulative CBOR size of transactions held in the mempool")
                .with_unit("bytes")
                .build()
        });
        let tx_count = TX_COUNT.get_or_init(|| {
            meter
                .u64_gauge("cardano_node_mempool_tx_count")
                .with_description("current number of transactions held in the mempool")
                .with_unit("int")
                .build()
        });
        let tx_insertions = TX_INSERTIONS.get_or_init(|| {
            meter
                .u64_counter("cardano_node_mempool_tx_insertions_total")
                .with_description("transaction insertion attempts, labelled by origin and result")
                .with_unit("int")
                .build()
        });
        let tx_evicted = TX_EVICTED.get_or_init(|| {
            meter
                .u64_counter("cardano_node_mempool_tx_evicted_total")
                .with_description("transactions removed from the mempool, labelled by reason")
                .with_unit("int")
                .build()
        });
        let revalidations = REVALIDATIONS.get_or_init(|| {
            meter
                .u64_counter("cardano_node_mempool_revalidations_total")
                .with_description("revalidation passes triggered by tip changes")
                .with_unit("int")
                .build()
        });

        size_bytes.record(self.size_bytes, &[]);
        tx_count.record(self.tx_count, &[]);

        match self.event {
            MempoolMetricEvent::TxInsertion { origin, result } => {
                tx_insertions
                    .add(1, &[KeyValue::new("origin", origin.as_label()), KeyValue::new("result", result.as_label())]);
            }
            MempoolMetricEvent::TxEvicted { reason } => {
                tx_evicted.add(1, &[KeyValue::new("reason", reason.as_label())]);
            }
            MempoolMetricEvent::Revalidated => {
                revalidations.add(1, &[]);
            }
        }
    }
}

impl From<MempoolMetrics> for MetricsEvent {
    fn from(value: MempoolMetrics) -> Self {
        MetricsEvent::MempoolMetrics(value)
    }
}
