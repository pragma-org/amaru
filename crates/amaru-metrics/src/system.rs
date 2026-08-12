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
use crate::Gauge;
use crate::{Meter, MetricRecorder, MetricsEvent};

pub const PROCESS_RUNTIME: &str = "process_runtime";
pub const PROCESS_CPU_LIVE: &str = "process_cpu_live";
pub const PROCESS_DISK_TOTAL_READ: &str = "process_disk_total_read";
pub const PROCESS_DISK_TOTAL_WRITE: &str = "process_disk_total_write";
pub const PROCESS_DISK_LIVE_READ: &str = "process_disk_live_read";
pub const PROCESS_DISK_LIVE_WRITE: &str = "process_disk_live_write";
pub const PROCESS_MEMORY_FOOTPRINT: &str = "process_memory_footprint";
pub const PROCESS_MEMORY_LIVE_RESIDENT: &str = "process_memory_live_resident";
pub const PROCESS_MEMORY_AVAILABLE_VIRTUAL: &str = "process_memory_available_virtual";
pub const PROCESS_OPEN_FILES: &str = "process_open_files";
pub const HOST_MEMORY_USED: &str = "host_memory_used";
pub const HOST_MEMORY_TOTAL: &str = "host_memory_total";
pub const HOST_DISK_LIVE_READ: &str = "host_disk_live_read";
pub const HOST_DISK_LIVE_WRITE: &str = "host_disk_live_write";

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SystemMetrics {
    pub node_start_time_seconds: u64,
    pub cpu_ticks: u64,
    pub block_io_ticks: u64,
    pub network_read_bytes: u64,
    pub network_written_bytes: u64,
    pub runtime_seconds: u64,
    pub cpu_percent: f64,
    pub process_memory_bytes: u64,
    pub process_memory_live_resident: u64,
    pub process_memory_available_virtual: u64,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    pub disk_read_bytes: u64,
    pub disk_write_bytes: u64,
    pub disk_live_read_bytes: u64,
    pub disk_live_write_bytes: u64,
    pub host_live_read_bytes: u64,
    pub host_live_write_bytes: u64,
    pub open_files: u64,
}

#[cfg(target_arch = "wasm32")]
impl MetricRecorder for SystemMetrics {
    fn record_to_meter(&self, _meter: &Meter) {}
}

#[cfg(not(target_arch = "wasm32"))]
impl MetricRecorder for SystemMetrics {
    fn record_to_meter(&self, meter: &Meter) {
        static NODE_START_TIME: OnceLock<Gauge<u64>> = OnceLock::new();
        static BASIC_INFO: OnceLock<Gauge<u64>> = OnceLock::new();
        static CPU_TICKS: OnceLock<Gauge<u64>> = OnceLock::new();
        static BLOCK_IO_TICKS: OnceLock<Gauge<u64>> = OnceLock::new();
        static NETWORK_READ_BYTES: OnceLock<Gauge<u64>> = OnceLock::new();
        static NETWORK_WRITTEN_BYTES: OnceLock<Gauge<u64>> = OnceLock::new();
        static FILESYSTEM_READ_BYTES: OnceLock<Gauge<u64>> = OnceLock::new();
        static FILESYSTEM_WRITTEN_BYTES: OnceLock<Gauge<u64>> = OnceLock::new();
        static MEMORY_RESIDENT_BYTES: OnceLock<Gauge<u64>> = OnceLock::new();
        static RUNTIME_SECONDS: OnceLock<Gauge<u64>> = OnceLock::new();
        static DISK_TOTAL_READ_BYTES: OnceLock<Gauge<u64>> = OnceLock::new();
        static DISK_TOTAL_WRITE_BYTES: OnceLock<Gauge<u64>> = OnceLock::new();
        static DISK_LIVE_READ_BYTES: OnceLock<Gauge<u64>> = OnceLock::new();
        static DISK_LIVE_WRITE_BYTES: OnceLock<Gauge<u64>> = OnceLock::new();
        static HOST_DISK_LIVE_READ_BYTES: OnceLock<Gauge<u64>> = OnceLock::new();
        static HOST_DISK_LIVE_WRITE_BYTES: OnceLock<Gauge<u64>> = OnceLock::new();
        static CPU_PERCENT: OnceLock<Gauge<f64>> = OnceLock::new();
        static FOOTPRINT_BYTES: OnceLock<Gauge<u64>> = OnceLock::new();
        static RSS_BYTES: OnceLock<Gauge<u64>> = OnceLock::new();
        static VIRTUAL_BYTES: OnceLock<Gauge<u64>> = OnceLock::new();
        static HOST_MEMORY_USED_BYTES: OnceLock<Gauge<u64>> = OnceLock::new();
        static HOST_MEMORY_TOTAL_BYTES: OnceLock<Gauge<u64>> = OnceLock::new();
        static OPEN_FILES: OnceLock<Gauge<u64>> = OnceLock::new();

        let Some(meter) = meter.get() else {
            return;
        };

        let node_start_time = NODE_START_TIME.get_or_init(|| {
            meter
                .u64_gauge("cardano_node_metrics_node_start_time_int")
                .with_description("node start time as seconds since the Unix epoch")
                .with_unit("seconds")
                .build()
        });
        let basic_info = BASIC_INFO.get_or_init(|| {
            meter
                .u64_gauge("cardano_node_metrics_basicInfo")
                .with_description("basic information for the running Amaru node")
                .build()
        });
        let cpu_ticks = CPU_TICKS.get_or_init(|| {
            meter
                .u64_gauge("cardano_node_metrics_Stat_cputicks_int")
                .with_description("total CPU time used by the process in centiseconds")
                .with_unit("centiseconds")
                .build()
        });
        let block_io_ticks = BLOCK_IO_TICKS.get_or_init(|| {
            meter
                .u64_gauge("cardano_node_metrics_Stat_blkIOticks_int")
                .with_description("total time the process has waited for block I/O in centiseconds")
                .with_unit("centiseconds")
                .build()
        });
        let network_read_bytes = NETWORK_READ_BYTES.get_or_init(|| {
            meter
                .u64_gauge("cardano_node_metrics_Stat_netRd_int")
                .with_description("total bytes received by the host's network interfaces")
                .with_unit("bytes")
                .build()
        });
        let network_written_bytes = NETWORK_WRITTEN_BYTES.get_or_init(|| {
            meter
                .u64_gauge("cardano_node_metrics_Stat_netWr_int")
                .with_description("total bytes sent by the host's network interfaces")
                .with_unit("bytes")
                .build()
        });
        let filesystem_read_bytes = FILESYSTEM_READ_BYTES.get_or_init(|| {
            meter
                .u64_gauge("cardano_node_metrics_Stat_fsRd_int")
                .with_description("total bytes read from storage by the process")
                .with_unit("bytes")
                .build()
        });
        let filesystem_written_bytes = FILESYSTEM_WRITTEN_BYTES.get_or_init(|| {
            meter
                .u64_gauge("cardano_node_metrics_Stat_fsWr_int")
                .with_description("total bytes written to storage by the process")
                .with_unit("bytes")
                .build()
        });
        let memory_resident_bytes = MEMORY_RESIDENT_BYTES.get_or_init(|| {
            meter
                .u64_gauge("cardano_node_metrics_Mem_resident_int")
                .with_description("kernel-reported resident set size")
                .with_unit("bytes")
                .build()
        });
        let runtime_seconds = RUNTIME_SECONDS.get_or_init(|| {
            meter
                .u64_gauge(PROCESS_RUNTIME)
                .with_description("How much time the process has been running (in seconds)")
                .with_unit("seconds")
                .build()
        });
        let disk_total_read_bytes = DISK_TOTAL_READ_BYTES.get_or_init(|| {
            meter
                .u64_gauge(PROCESS_DISK_TOTAL_READ)
                .with_description("Total number of read bytes (in bytes).")
                .with_unit("bytes")
                .build()
        });
        let disk_total_write_bytes = DISK_TOTAL_WRITE_BYTES.get_or_init(|| {
            meter
                .u64_gauge(PROCESS_DISK_TOTAL_WRITE)
                .with_description("Total number of written bytes (in bytes).")
                .with_unit("bytes")
                .build()
        });
        let cpu_percent = CPU_PERCENT.get_or_init(|| {
            meter.f64_gauge(PROCESS_CPU_LIVE).with_description("Current CPU utilization (in %)").with_unit("%").build()
        });
        let disk_live_read_bytes = DISK_LIVE_READ_BYTES.get_or_init(|| {
            meter
                .u64_gauge(PROCESS_DISK_LIVE_READ)
                .with_description("Number of read bytes since the last refresh (in bytes)")
                .with_unit("bytes")
                .build()
        });
        let disk_live_write_bytes = DISK_LIVE_WRITE_BYTES.get_or_init(|| {
            meter
                .u64_gauge(PROCESS_DISK_LIVE_WRITE)
                .with_description("Number of written bytes since the last refresh (in bytes)")
                .with_unit("bytes")
                .build()
        });
        let host_disk_live_read_bytes = HOST_DISK_LIVE_READ_BYTES.get_or_init(|| {
            meter
                .u64_gauge(HOST_DISK_LIVE_READ)
                .with_description("Number of read bytes observed across the host since the last refresh (in bytes)")
                .with_unit("bytes")
                .build()
        });
        let host_disk_live_write_bytes = HOST_DISK_LIVE_WRITE_BYTES.get_or_init(|| {
            meter
                .u64_gauge(HOST_DISK_LIVE_WRITE)
                .with_description("Number of written bytes observed across the host since the last refresh (in bytes)")
                .with_unit("bytes")
                .build()
        });
        let rss_bytes = RSS_BYTES.get_or_init(|| {
            meter
                .u64_gauge(PROCESS_MEMORY_LIVE_RESIDENT)
                .with_description(
                    "The amount of memory that the process allocated and which is currently mapped in physical RAM (in bytes).",
                )
                .with_unit("bytes")
                .build()
        });
        let footprint_bytes = FOOTPRINT_BYTES.get_or_init(|| {
            meter
                .u64_gauge(PROCESS_MEMORY_FOOTPRINT)
                .with_description(
                    "Process memory footprint as reported by the local host tooling when available, falling back to the resident set size otherwise (in bytes).",
                )
                .with_unit("bytes")
                .build()
        });
        let virtual_bytes = VIRTUAL_BYTES.get_or_init(|| {
            meter
                .u64_gauge(PROCESS_MEMORY_AVAILABLE_VIRTUAL)
                .with_description(
                    "The amount of memory that the process can access, whether it is currently mapped in physical RAM or not (in bytes).",
                )
                .with_unit("bytes")
                .build()
        });
        let host_memory_used_bytes = HOST_MEMORY_USED_BYTES.get_or_init(|| {
            meter
                .u64_gauge(HOST_MEMORY_USED)
                .with_description("Amount of RAM currently used on the host (in bytes)")
                .with_unit("bytes")
                .build()
        });
        let host_memory_total_bytes = HOST_MEMORY_TOTAL_BYTES.get_or_init(|| {
            meter
                .u64_gauge(HOST_MEMORY_TOTAL)
                .with_description("Total amount of RAM available on the host (in bytes)")
                .with_unit("bytes")
                .build()
        });
        let open_files = OPEN_FILES.get_or_init(|| {
            meter.u64_gauge(PROCESS_OPEN_FILES).with_description("Total number of file descriptors.").build()
        });

        node_start_time.record(self.node_start_time_seconds, &[]);
        basic_info.record(1, &[KeyValue::new("nodeStartTime", self.node_start_time_seconds.to_string())]);
        cpu_ticks.record(self.cpu_ticks, &[]);
        block_io_ticks.record(self.block_io_ticks, &[]);
        network_read_bytes.record(self.network_read_bytes, &[]);
        network_written_bytes.record(self.network_written_bytes, &[]);
        filesystem_read_bytes.record(self.disk_read_bytes, &[]);
        filesystem_written_bytes.record(self.disk_write_bytes, &[]);
        memory_resident_bytes.record(self.process_memory_live_resident, &[]);
        runtime_seconds.record(self.runtime_seconds, &[]);
        disk_total_read_bytes.record(self.disk_read_bytes, &[]);
        disk_total_write_bytes.record(self.disk_write_bytes, &[]);
        disk_live_read_bytes.record(self.disk_live_read_bytes, &[]);
        disk_live_write_bytes.record(self.disk_live_write_bytes, &[]);
        host_disk_live_read_bytes.record(self.host_live_read_bytes, &[]);
        host_disk_live_write_bytes.record(self.host_live_write_bytes, &[]);
        cpu_percent.record(self.cpu_percent, &[]);
        footprint_bytes.record(self.process_memory_bytes, &[]);
        rss_bytes.record(self.process_memory_live_resident, &[]);
        virtual_bytes.record(self.process_memory_available_virtual, &[]);
        host_memory_used_bytes.record(self.memory_used_bytes, &[]);
        host_memory_total_bytes.record(self.memory_total_bytes, &[]);
        open_files.record(self.open_files, &[]);
    }
}

impl From<SystemMetrics> for MetricsEvent {
    fn from(value: SystemMetrics) -> Self {
        MetricsEvent::SystemMetrics(value)
    }
}
