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
use crate::{Counter, Gauge};
use crate::{Meter, MetricRecorder, MetricsEvent};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ProtocolMetrics {
    ConnectionManager(ConnectionManagerMetrics),
    ServedBlockCount(ServedBlockCountMetrics),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ConnectionManagerMetrics {
    pub duplex_connections: u64,
    pub full_duplex_connections: u64,
    pub inbound_connections: u64,
    pub outbound_connections: u64,
    pub unidirectional_connections: u64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ServedBlockCountMetrics {
    pub count: u64,
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
        static DUPLEX_CONNECTIONS: OnceLock<Gauge<u64>> = OnceLock::new();
        static FULL_DUPLEX_CONNECTIONS: OnceLock<Gauge<u64>> = OnceLock::new();
        static INBOUND_CONNECTIONS: OnceLock<Gauge<u64>> = OnceLock::new();
        static OUTBOUND_CONNECTIONS: OnceLock<Gauge<u64>> = OnceLock::new();
        static UNIDIRECTIONAL_CONNECTIONS: OnceLock<Gauge<u64>> = OnceLock::new();

        let duplex_connections = DUPLEX_CONNECTIONS.get_or_init(|| {
            meter
                .u64_gauge("cardano_node_metrics_connectionManager_duplexConns_int")
                .with_description("current number of duplex connections")
                .with_unit("int")
                .build()
        });
        let full_duplex_connections = FULL_DUPLEX_CONNECTIONS.get_or_init(|| {
            meter
                .u64_gauge("cardano_node_metrics_connectionManager_fullDuplexConns_int")
                .with_description("current number of full duplex connections")
                .with_unit("int")
                .build()
        });
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

        duplex_connections.record(self.duplex_connections, &[]);
        full_duplex_connections.record(self.full_duplex_connections, &[]);
        inbound_connections.record(self.inbound_connections, &[]);
        outbound_connections.record(self.outbound_connections, &[]);
        unidirectional_connections.record(self.unidirectional_connections, &[]);
    }
}

#[cfg(target_arch = "wasm32")]
impl MetricRecorder for ServedBlockCountMetrics {
    fn record_to_meter(&self, _meter: &Meter) {}
}

#[cfg(not(target_arch = "wasm32"))]
impl MetricRecorder for ServedBlockCountMetrics {
    fn record_to_meter(&self, meter: &Meter) {
        static SERVED_BLOCK_COUNT: OnceLock<Counter<u64>> = OnceLock::new();

        let served_block_count = SERVED_BLOCK_COUNT.get_or_init(|| {
            meter
                .u64_counter("cardano_node_metrics_served_block_count_int")
                .with_description("total number of blocks served to peers")
                .with_unit("int")
                .build()
        });

        served_block_count.add(self.count, &[]);
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
