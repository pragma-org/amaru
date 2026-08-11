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
use std::sync::{Arc, Mutex, OnceLock};

use amaru_kernel::Slot;
#[cfg(not(target_arch = "wasm32"))]
use opentelemetry::KeyValue;

#[cfg(not(target_arch = "wasm32"))]
use crate::{Counter, Gauge};
use crate::{Meter, MetricRecorder, MetricsEvent};

#[cfg(not(target_arch = "wasm32"))]
static SERVED_BLOCK_COUNT: OnceLock<Counter<u64>> = OnceLock::new();
#[cfg(not(target_arch = "wasm32"))]
static SERVED_BLOCK_LATEST: OnceLock<Gauge<u64>> = OnceLock::new();
#[cfg(not(target_arch = "wasm32"))]
static SERVED_HEADER: OnceLock<Counter<u64>> = OnceLock::new();
#[cfg(not(target_arch = "wasm32"))]
static CHAIN_SYNC_HEADERS_SERVED: OnceLock<Counter<u64>> = OnceLock::new();

#[cfg(not(target_arch = "wasm32"))]
fn served_block_count(meter: &opentelemetry::metrics::Meter) -> &'static Counter<u64> {
    SERVED_BLOCK_COUNT.get_or_init(|| {
        meter
            .u64_counter("cardano_node_metrics_served_block_counter")
            .with_description("total number of blocks served to peers")
            .with_unit("int")
            .build()
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn served_block_latest(meter: &opentelemetry::metrics::Meter) -> &'static Gauge<u64> {
    SERVED_BLOCK_LATEST.get_or_init(|| {
        meter
            .u64_gauge("cardano_node_metrics_served_block_latest_int")
            .with_description("number of blocks served at the highest slot observed so far")
            .with_unit("int")
            .build()
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn served_header(meter: &opentelemetry::metrics::Meter) -> &'static Counter<u64> {
    SERVED_HEADER.get_or_init(|| {
        meter
            .u64_counter("cardano_node_metrics_served_header_counter")
            .with_description("total number of headers served to peers")
            .with_unit("int")
            .build()
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn chain_sync_headers_served(meter: &opentelemetry::metrics::Meter) -> &'static Counter<u64> {
    CHAIN_SYNC_HEADERS_SERVED.get_or_init(|| {
        meter
            .u64_counter("cardano_node_metrics_ChainSync_HeadersServed_counter")
            .with_description("total number of headers served through chain sync")
            .with_unit("int")
            .build()
    })
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn initialize_metrics(meter: &Meter) {
    let Some(meter) = meter.get() else {
        return;
    };

    served_block_count(meter).add(0, &[]);
    served_block_latest(meter).record(0, &[]);
    served_header(meter).add(0, &[]);
    chain_sync_headers_served(meter).add(0, &[]);
}

// Uses ObservableGauge rather than Gauge: the callback output replaces the previous
// observation set each collection cycle, so only the current label set is ever exported.
// A synchronous Gauge would accumulate every distinct label combination seen since startup.
//
// `state` and `gauge` must be `'static` at the call site: `state` is the shared cell
// read by the callback; `gauge` keeps the handle alive (dropping it deregisters the callback).
#[cfg(not(target_arch = "wasm32"))]
fn update_observable_gauge<T>(
    state: &'static OnceLock<Arc<Mutex<Option<T>>>>,
    gauge: &'static OnceLock<opentelemetry::metrics::ObservableGauge<u64>>,
    meter: &Meter,
    name: &'static str,
    description: &'static str,
    current: T,
    attrs: impl Fn(&T) -> Vec<KeyValue> + Send + Sync + 'static,
) where
    T: Clone + Send + 'static,
{
    let Some(meter) = meter.get() else {
        return;
    };

    let state_ref = state.get_or_init(|| {
        let shared: Arc<Mutex<Option<T>>> = Arc::new(Mutex::new(None));
        let shared_cb = shared.clone();
        gauge.get_or_init(|| {
            meter
                .u64_observable_gauge(name)
                .with_description(description)
                .with_callback(move |observer| {
                    if let Ok(guard) = shared_cb.lock()
                        && let Some(value) = guard.as_ref()
                    {
                        observer.observe(1, &attrs(value));
                    }
                })
                .build()
        });
        shared
    });

    if let Ok(mut guard) = state_ref.lock() {
        *guard = Some(current);
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ProtocolMetrics {
    ConnectionManager(ConnectionManagerMetrics),
    ServedBlockCount(ServedBlockCountMetrics),
    TipBlock(TipBlockMetrics),
    ServedBlockLatest(ServedBlockLatestMetrics),
    ServedHeader(ServedHeaderMetrics),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ConnectionManagerMetrics {
    pub inbound_connections: u64,
    pub outbound_connections: u64,
    pub unidirectional_connections: u64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ServedBlockCountMetrics {
    pub count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ServedBlockLatestMetrics {
    pub slot: Slot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ServedHeaderMetrics {
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TipBlockMetrics {
    pub hash: String,
    pub parent_hash: String,
    pub issuer_verification_key_hash: String,
}

#[cfg(target_arch = "wasm32")]
impl MetricRecorder for ProtocolMetrics {
    fn record_to_meter(&self, _meter: &Meter) {}
}

#[cfg(not(target_arch = "wasm32"))]
impl MetricRecorder for ProtocolMetrics {
    fn record_to_meter(&self, meter: &Meter) {
        match self {
            ProtocolMetrics::ConnectionManager(metrics) => metrics.record_to_meter(meter),
            ProtocolMetrics::ServedBlockCount(metrics) => metrics.record_to_meter(meter),
            ProtocolMetrics::TipBlock(metrics) => metrics.record_to_meter(meter),
            ProtocolMetrics::ServedBlockLatest(metrics) => metrics.record_to_meter(meter),
            ProtocolMetrics::ServedHeader(metrics) => metrics.record_to_meter(meter),
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl MetricRecorder for ConnectionManagerMetrics {
    fn record_to_meter(&self, _meter: &Meter) {}
}

#[cfg(not(target_arch = "wasm32"))]
impl MetricRecorder for ConnectionManagerMetrics {
    fn record_to_meter(&self, meter: &Meter) {
        static INBOUND_CONNECTIONS: OnceLock<Gauge<u64>> = OnceLock::new();
        static OUTBOUND_CONNECTIONS: OnceLock<Gauge<u64>> = OnceLock::new();
        static UNIDIRECTIONAL_CONNECTIONS: OnceLock<Gauge<u64>> = OnceLock::new();

        let Some(meter) = meter.get() else {
            return;
        };

        let inbound_connections = INBOUND_CONNECTIONS.get_or_init(|| {
            meter
                .u64_gauge("cardano_node_metrics_connectionManager_inboundConns_int")
                .with_description("current number of inbound connections")
                .with_unit("int")
                .build()
        });
        let outbound_connections = OUTBOUND_CONNECTIONS.get_or_init(|| {
            meter
                .u64_gauge("cardano_node_metrics_connectionManager_outboundConns_int")
                .with_description("current number of outbound connections")
                .with_unit("int")
                .build()
        });
        let unidirectional_connections = UNIDIRECTIONAL_CONNECTIONS.get_or_init(|| {
            meter
                .u64_gauge("cardano_node_metrics_connectionManager_unidirectionalConns_int")
                .with_description("current number of unidirectional connections")
                .with_unit("int")
                .build()
        });

        inbound_connections.record(self.inbound_connections, &[]);
        outbound_connections.record(self.outbound_connections, &[]);
        unidirectional_connections.record(self.unidirectional_connections, &[]);
    }
}

#[cfg(target_arch = "wasm32")]
impl MetricRecorder for ServedBlockCountMetrics {
    fn record_to_meter(&self, _meter: &Meter) {}
}

#[cfg(target_arch = "wasm32")]
impl MetricRecorder for ServedBlockLatestMetrics {
    fn record_to_meter(&self, _meter: &Meter) {}
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
struct ServedBlockLatestState {
    max_slot: Slot,
    count: u64,
}

#[cfg(not(target_arch = "wasm32"))]
impl ServedBlockLatestState {
    fn observe(&mut self, slot: Slot) -> u64 {
        if slot > self.max_slot {
            self.max_slot = slot;
            self.count = 1;
        } else if slot == self.max_slot {
            self.count = self.count.saturating_add(1);
        }
        self.count
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl MetricRecorder for ServedBlockLatestMetrics {
    fn record_to_meter(&self, meter: &Meter) {
        static STATE: OnceLock<Mutex<ServedBlockLatestState>> = OnceLock::new();

        let Some(meter) = meter.get() else {
            return;
        };

        if let Ok(mut state) = STATE.get_or_init(Default::default).lock() {
            served_block_latest(meter).record(state.observe(self.slot), &[]);
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl MetricRecorder for ServedHeaderMetrics {
    fn record_to_meter(&self, _meter: &Meter) {}
}

#[cfg(not(target_arch = "wasm32"))]
impl MetricRecorder for ServedHeaderMetrics {
    fn record_to_meter(&self, meter: &Meter) {
        let Some(meter) = meter.get() else {
            return;
        };

        served_header(meter).add(self.count, &[]);
        chain_sync_headers_served(meter).add(self.count, &[]);
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl MetricRecorder for ServedBlockCountMetrics {
    fn record_to_meter(&self, meter: &Meter) {
        let Some(meter) = meter.get() else {
            return;
        };

        served_block_count(meter).add(self.count, &[]);
    }
}

#[cfg(target_arch = "wasm32")]
impl MetricRecorder for TipBlockMetrics {
    fn record_to_meter(&self, _meter: &Meter) {}
}

#[cfg(not(target_arch = "wasm32"))]
impl MetricRecorder for TipBlockMetrics {
    fn record_to_meter(&self, meter: &Meter) {
        static CURRENT_TIP: OnceLock<Arc<Mutex<Option<TipBlockMetrics>>>> = OnceLock::new();
        static TIP_BLOCK_GAUGE: OnceLock<opentelemetry::metrics::ObservableGauge<u64>> = OnceLock::new();

        update_observable_gauge(
            &CURRENT_TIP,
            &TIP_BLOCK_GAUGE,
            meter,
            "cardano_node_metrics_tipBlock",
            "current chain tip block info",
            self.clone(),
            |tip| {
                vec![
                    KeyValue::new("hash", tip.hash.clone()),
                    KeyValue::new("parent_hash", tip.parent_hash.clone()),
                    KeyValue::new("issuer_verification_key_hash", tip.issuer_verification_key_hash.clone()),
                ]
            },
        );
    }
}

impl From<ProtocolMetrics> for MetricsEvent {
    fn from(value: ProtocolMetrics) -> Self {
        MetricsEvent::ProtocolMetrics(value)
    }
}

impl From<ConnectionManagerMetrics> for MetricsEvent {
    fn from(value: ConnectionManagerMetrics) -> Self {
        MetricsEvent::ProtocolMetrics(ProtocolMetrics::ConnectionManager(value))
    }
}

impl From<ServedBlockCountMetrics> for MetricsEvent {
    fn from(value: ServedBlockCountMetrics) -> Self {
        MetricsEvent::ProtocolMetrics(ProtocolMetrics::ServedBlockCount(value))
    }
}

impl From<ServedBlockLatestMetrics> for MetricsEvent {
    fn from(value: ServedBlockLatestMetrics) -> Self {
        MetricsEvent::ProtocolMetrics(ProtocolMetrics::ServedBlockLatest(value))
    }
}

impl From<ServedHeaderMetrics> for MetricsEvent {
    fn from(value: ServedHeaderMetrics) -> Self {
        MetricsEvent::ProtocolMetrics(ProtocolMetrics::ServedHeader(value))
    }
}

impl From<TipBlockMetrics> for MetricsEvent {
    fn from(value: TipBlockMetrics) -> Self {
        MetricsEvent::ProtocolMetrics(ProtocolMetrics::TipBlock(value))
    }
}
