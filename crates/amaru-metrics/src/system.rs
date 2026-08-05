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
use std::sync::LazyLock;

#[cfg(not(target_arch = "wasm32"))]
use opentelemetry::KeyValue;

#[cfg(not(target_arch = "wasm32"))]
use crate::{Gauge, METRICS_METER};
use crate::{Meter, MetricRecorder, MetricsEvent};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SystemMetrics {
    pub node_start_time_seconds: u64,
    pub cpu_ticks: u64,
    pub network_read_bytes: u64,
    pub network_written_bytes: u64,
    pub runtime_seconds: u64,
    pub cpu_percent: f64,
    pub rss_bytes: u64,
    pub virtual_bytes: u64,
    pub disk_read_bytes: u64,
    pub disk_write_bytes: u64,
    pub disk_live_read_bytes: u64,
    pub disk_live_write_bytes: u64,
    pub open_files: u64,
}

#[cfg(target_arch = "wasm32")]
impl MetricRecorder for SystemMetrics {
    fn record_to_meter(&self, _meter: &Meter) {}
}

#[cfg(not(target_arch = "wasm32"))]
impl MetricRecorder for SystemMetrics {
    fn record_to_meter(&self, _meter: &Meter) {
        static NODE_START_TIME: LazyLock<Gauge<u64>> = LazyLock::new(|| {
            METRICS_METER
                .u64_gauge("cardano_node_metrics_node_start_time_int")
                .with_description("node start time as seconds since the Unix epoch")
                .with_unit("seconds")
                .build()
        });
        static BASIC_INFO: LazyLock<Gauge<u64>> = LazyLock::new(|| {
            METRICS_METER
                .u64_gauge("cardano_node_metrics_basicInfo")
                .with_description("basic information for the running Amaru node")
                .build()
        });
        static CPU_TICKS: LazyLock<Gauge<u64>> = LazyLock::new(|| {
            METRICS_METER
                .u64_gauge("cardano_node_metrics_Stat_cputicks_int")
                .with_description("total CPU time used by the process in centiseconds")
                .with_unit("centiseconds")
                .build()
        });
        static NETWORK_READ_BYTES: LazyLock<Gauge<u64>> = LazyLock::new(|| {
            METRICS_METER
                .u64_gauge("cardano_node_metrics_Stat_netRd_int")
                .with_description("total bytes received by the host's network interfaces")
                .with_unit("bytes")
                .build()
        });
        static NETWORK_WRITTEN_BYTES: LazyLock<Gauge<u64>> = LazyLock::new(|| {
            METRICS_METER
                .u64_gauge("cardano_node_metrics_Stat_netWr_int")
                .with_description("total bytes sent by the host's network interfaces")
                .with_unit("bytes")
                .build()
        });
        static FILESYSTEM_READ_BYTES: LazyLock<Gauge<u64>> = LazyLock::new(|| {
            METRICS_METER
                .u64_gauge("cardano_node_metrics_Stat_fsRd_int")
                .with_description("total bytes read from storage by the process")
                .with_unit("bytes")
                .build()
        });
        static FILESYSTEM_WRITTEN_BYTES: LazyLock<Gauge<u64>> = LazyLock::new(|| {
            METRICS_METER
                .u64_gauge("cardano_node_metrics_Stat_fsWr_int")
                .with_description("total bytes written to storage by the process")
                .with_unit("bytes")
                .build()
        });
        static MEMORY_RESIDENT_BYTES: LazyLock<Gauge<u64>> = LazyLock::new(|| {
            METRICS_METER
                .u64_gauge("cardano_node_metrics_Mem_resident_int")
                .with_description("kernel-reported resident set size")
                .with_unit("bytes")
                .build()
        });

        NODE_START_TIME.record(self.node_start_time_seconds, &[]);
        BASIC_INFO.record(1, &[KeyValue::new("nodeStartTime", self.node_start_time_seconds.to_string())]);
        CPU_TICKS.record(self.cpu_ticks, &[]);
        NETWORK_READ_BYTES.record(self.network_read_bytes, &[]);
        NETWORK_WRITTEN_BYTES.record(self.network_written_bytes, &[]);
        FILESYSTEM_READ_BYTES.record(self.disk_read_bytes, &[]);
        FILESYSTEM_WRITTEN_BYTES.record(self.disk_write_bytes, &[]);
        MEMORY_RESIDENT_BYTES.record(self.rss_bytes, &[]);
    }
}

impl From<SystemMetrics> for MetricsEvent {
    fn from(value: SystemMetrics) -> Self {
        MetricsEvent::SystemMetrics(value)
    }
}
