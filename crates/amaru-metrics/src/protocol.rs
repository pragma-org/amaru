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

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ProtocolMetrics {
    ConnectionManager(ConnectionManagerMetrics),
    ServedBlockCount(ServedBlockCountMetrics),
    ServedHeaderCount(ServedHeaderCountMetrics),
    TipBlock(TipBlockMetrics),
    BlockfetchClient(BlockfetchClientMetrics),
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

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ServedHeaderCountMetrics {
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TipBlockMetrics {
    pub hash: String,
    pub parent_hash: String,
    pub issuer_verification_key_hash: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BlockfetchClientMetrics {
    pub block_delay: f64,
    pub block_delay_cdf_one: f64,
    pub block_delay_cdf_three: f64,
    pub block_delay_cdf_five: f64,
    pub block_size: u64,
    pub late_blocks: u64,
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
            ProtocolMetrics::ServedHeaderCount(metrics) => metrics.record_to_meter(meter),
            ProtocolMetrics::TipBlock(metrics) => metrics.record_to_meter(meter),
            ProtocolMetrics::BlockfetchClient(metrics) => metrics.record_to_meter(meter),
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

#[cfg(target_arch = "wasm32")]
impl MetricRecorder for ServedHeaderCountMetrics {
    fn record_to_meter(&self, _meter: &Meter) {}
}

#[cfg(not(target_arch = "wasm32"))]
impl MetricRecorder for ServedHeaderCountMetrics {
    fn record_to_meter(&self, meter: &Meter) {
        static SERVED_HEADER_COUNT: OnceLock<Counter<u64>> = OnceLock::new();

        let served_header_count = SERVED_HEADER_COUNT.get_or_init(|| {
            meter
                .u64_counter("cardano_node_metrics_ChainSync_HeadersServed_counter")
                .with_description("total number of chain sync headers served to peers")
                .with_unit("count")
                .build()
        });

        served_header_count.add(self.count, &[]);
    }
}

#[cfg(target_arch = "wasm32")]
impl MetricRecorder for TipBlockMetrics {
    fn record_to_meter(&self, _meter: &Meter) {}
}

#[cfg(not(target_arch = "wasm32"))]
impl MetricRecorder for TipBlockMetrics {
    fn record_to_meter(&self, meter: &Meter) {
        static TIP_BLOCK: OnceLock<Gauge<u64>> = OnceLock::new();

        let tip_block = TIP_BLOCK.get_or_init(|| {
            meter
                .u64_gauge("cardano_node_metrics_tipBlock")
                .with_description("current adopted tip block identity")
                .build()
        });

        tip_block.record(
            1,
            &[
                KeyValue::new("hash", self.hash.clone()),
                KeyValue::new("parent_hash", self.parent_hash.clone()),
                KeyValue::new("issuer_verification_key_hash", self.issuer_verification_key_hash.clone()),
            ],
        );
    }
}

#[cfg(target_arch = "wasm32")]
impl MetricRecorder for BlockfetchClientMetrics {
    fn record_to_meter(&self, _meter: &Meter) {}
}

#[cfg(not(target_arch = "wasm32"))]
impl MetricRecorder for BlockfetchClientMetrics {
    fn record_to_meter(&self, meter: &Meter) {
        static BLOCK_DELAY: OnceLock<Gauge<f64>> = OnceLock::new();
        static BLOCK_DELAY_CDF_ONE: OnceLock<Gauge<f64>> = OnceLock::new();
        static BLOCK_DELAY_CDF_THREE: OnceLock<Gauge<f64>> = OnceLock::new();
        static BLOCK_DELAY_CDF_FIVE: OnceLock<Gauge<f64>> = OnceLock::new();
        static BLOCK_SIZE: OnceLock<Gauge<u64>> = OnceLock::new();
        static LATE_BLOCKS: OnceLock<Counter<u64>> = OnceLock::new();

        let block_delay = BLOCK_DELAY.get_or_init(|| {
            meter
                .f64_gauge("cardano_node_metrics_blockfetchclient_blockdelay_real")
                .with_description("delay in seconds between a block's slot time and when it is received")
                .with_unit("real")
                .build()
        });
        let block_delay_cdf_one = BLOCK_DELAY_CDF_ONE.get_or_init(|| {
            meter
                .f64_gauge("cardano_node_metrics_blockfetchclient_blockdelay_cdfOne_real")
                .with_description("fraction of fetched blocks received within 1 second of their slot time")
                .with_unit("real")
                .build()
        });
        let block_delay_cdf_three = BLOCK_DELAY_CDF_THREE.get_or_init(|| {
            meter
                .f64_gauge("cardano_node_metrics_blockfetchclient_blockdelay_cdfThree_real")
                .with_description("fraction of fetched blocks received within 3 seconds of their slot time")
                .with_unit("real")
                .build()
        });
        let block_delay_cdf_five = BLOCK_DELAY_CDF_FIVE.get_or_init(|| {
            meter
                .f64_gauge("cardano_node_metrics_blockfetchclient_blockdelay_cdfFive_real")
                .with_description("fraction of fetched blocks received within 5 seconds of their slot time")
                .with_unit("real")
                .build()
        });
        let block_size = BLOCK_SIZE.get_or_init(|| {
            meter
                .u64_gauge("cardano_node_metrics_blockfetchclient_blocksize_int")
                .with_description("size in bytes of the most recently fetched block")
                .with_unit("int")
                .build()
        });
        let late_blocks = LATE_BLOCKS.get_or_init(|| {
            meter
                .u64_counter("cardano_node_metrics_blockfetchclient_lateblocks_counter")
                .with_description("total number of blocks received more than 5 seconds after their slot time")
                .with_unit("count")
                .build()
        });

        block_delay.record(self.block_delay, &[]);
        block_delay_cdf_one.record(self.block_delay_cdf_one, &[]);
        block_delay_cdf_three.record(self.block_delay_cdf_three, &[]);
        block_delay_cdf_five.record(self.block_delay_cdf_five, &[]);
        block_size.record(self.block_size, &[]);
        late_blocks.add(self.late_blocks, &[]);
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

impl From<ServedHeaderCountMetrics> for MetricsEvent {
    fn from(value: ServedHeaderCountMetrics) -> Self {
        MetricsEvent::ProtocolMetrics(ProtocolMetrics::ServedHeaderCount(value))
    }
}

impl From<TipBlockMetrics> for MetricsEvent {
    fn from(value: TipBlockMetrics) -> Self {
        MetricsEvent::ProtocolMetrics(ProtocolMetrics::TipBlock(value))
    }
}

impl From<BlockfetchClientMetrics> for MetricsEvent {
    fn from(value: BlockfetchClientMetrics) -> Self {
        MetricsEvent::ProtocolMetrics(ProtocolMetrics::BlockfetchClient(value))
    }
}
