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

use std::sync::OnceLock;

use crate::{Gauge, Meter, MetricRecorder, MetricsEvent};

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

impl MetricRecorder for SystemMetrics {
    fn record_to_meter(&self, meter: &Meter) {
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
