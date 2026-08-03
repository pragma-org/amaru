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
use crate::Gauge;
use crate::{Meter, MetricRecorder, MetricsEvent};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SystemMetrics {
    pub runtime_seconds: u64,
    pub cpu_percent: f64,
    pub process_memory_bytes: u64,
    pub rss_bytes: u64,
    pub virtual_bytes: u64,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    pub disk_read_bytes: u64,
    pub disk_write_bytes: u64,
    pub disk_live_read_bytes: u64,
    pub disk_live_write_bytes: u64,
    pub processes_live_read_bytes: u64,
    pub processes_live_write_bytes: u64,
    pub open_files: u64,
}

#[cfg(target_arch = "wasm32")]
impl MetricRecorder for SystemMetrics {
    fn record_to_meter(&self, _meter: &Meter) {}
}

#[cfg(not(target_arch = "wasm32"))]
impl MetricRecorder for SystemMetrics {
    fn record_to_meter(&self, meter: &Meter) {
        static RUNTIME_SECONDS: OnceLock<Gauge<u64>> = OnceLock::new();
        static DISK_TOTAL_READ_BYTES: OnceLock<Gauge<u64>> = OnceLock::new();
        static DISK_TOTAL_WRITE_BYTES: OnceLock<Gauge<u64>> = OnceLock::new();
        static DISK_LIVE_READ_BYTES: OnceLock<Gauge<u64>> = OnceLock::new();
        static DISK_LIVE_WRITE_BYTES: OnceLock<Gauge<u64>> = OnceLock::new();
        static PROCESSES_LIVE_READ_BYTES: OnceLock<Gauge<u64>> = OnceLock::new();
        static PROCESSES_LIVE_WRITE_BYTES: OnceLock<Gauge<u64>> = OnceLock::new();
        static CPU_PERCENT: OnceLock<Gauge<f64>> = OnceLock::new();
        static PROCESS_MEMORY_BYTES: OnceLock<Gauge<u64>> = OnceLock::new();
        static RSS_BYTES: OnceLock<Gauge<u64>> = OnceLock::new();
        static VIRTUAL_BYTES: OnceLock<Gauge<u64>> = OnceLock::new();
        static MEMORY_USED_BYTES: OnceLock<Gauge<u64>> = OnceLock::new();
        static MEMORY_TOTAL_BYTES: OnceLock<Gauge<u64>> = OnceLock::new();
        static OPEN_FILES: OnceLock<Gauge<u64>> = OnceLock::new();

        let runtime_seconds = RUNTIME_SECONDS.get_or_init(|| {
            meter
                .u64_gauge("process_runtime")
                .with_description("How much time the process has been running (in seconds)")
                .with_unit("seconds")
                .build()
        });
        let disk_total_read_bytes = DISK_TOTAL_READ_BYTES.get_or_init(|| {
            meter
                .u64_gauge("process_disk_total_read")
                .with_description("Total number of read bytes (in bytes).")
                .with_unit("bytes")
                .build()
        });
        let disk_total_write_bytes = DISK_TOTAL_WRITE_BYTES.get_or_init(|| {
            meter
                .u64_gauge("process_disk_total_write")
                .with_description("Total number of written bytes (in bytes).")
                .with_unit("bytes")
                .build()
        });
        let cpu_percent = CPU_PERCENT.get_or_init(|| {
            meter
                .f64_gauge("process_cpu_live")
                .with_description("Current CPU utilization (in %)")
                .with_unit("%")
                .build()
        });
        let disk_live_read_bytes = DISK_LIVE_READ_BYTES.get_or_init(|| {
            meter
                .u64_gauge("process_disk_live_read")
                .with_description("Number of read bytes since the last refresh (in bytes)")
                .with_unit("bytes")
                .build()
        });
        let disk_live_write_bytes = DISK_LIVE_WRITE_BYTES.get_or_init(|| {
            meter
                .u64_gauge("process_disk_live_write")
                .with_description("Number of written bytes since the last refresh (in bytes)")
                .with_unit("bytes")
                .build()
        });
        let processes_live_read_bytes = PROCESSES_LIVE_READ_BYTES.get_or_init(|| {
            meter
                .u64_gauge("amaru_metrics_processes_live_read_bytes")
                .with_description("Observed bytes read by all refreshed processes since the last refresh (in bytes)")
                .with_unit("bytes")
                .build()
        });
        let processes_live_write_bytes = PROCESSES_LIVE_WRITE_BYTES.get_or_init(|| {
            meter
                .u64_gauge("amaru_metrics_processes_live_write_bytes")
                .with_description("Observed bytes written by all refreshed processes since the last refresh (in bytes)")
                .with_unit("bytes")
                .build()
        });
        let process_memory_bytes = PROCESS_MEMORY_BYTES.get_or_init(|| {
            meter
                .u64_gauge("amaru_metrics_process_memory_footprint_bytes")
                .with_description("Current process memory footprint (in bytes)")
                .with_unit("bytes")
                .build()
        });
        let rss_bytes = RSS_BYTES.get_or_init(|| {
            meter
                .u64_gauge("process_memory_live_resident")
                .with_description(
                    "The amount of memory that the process allocated and which is currently mapped in physical RAM (in bytes).",
                )
                .with_unit("bytes")
                .build()
        });
        let virtual_bytes = VIRTUAL_BYTES.get_or_init(|| {
            meter
                .u64_gauge("process_memory_available_virtual")
                .with_description(
                    "The amount of memory that the process can access, whether it is currently mapped in physical RAM or not (in bytes).",
                )
                .with_unit("bytes")
                .build()
        });
        let memory_used_bytes = MEMORY_USED_BYTES.get_or_init(|| {
            meter
                .u64_gauge("amaru_metrics_system_memory_used_bytes")
                .with_description("Current system memory usage (in bytes)")
                .with_unit("bytes")
                .build()
        });
        let memory_total_bytes = MEMORY_TOTAL_BYTES.get_or_init(|| {
            meter
                .u64_gauge("amaru_metrics_system_memory_total_bytes")
                .with_description("Total system memory (in bytes)")
                .with_unit("bytes")
                .build()
        });
        let open_files = OPEN_FILES.get_or_init(|| {
            meter.u64_gauge("process_open_files").with_description("Total number of file descriptors.").build()
        });

        runtime_seconds.record(self.runtime_seconds, &[]);
        disk_total_read_bytes.record(self.disk_read_bytes, &[]);
        disk_total_write_bytes.record(self.disk_write_bytes, &[]);
        disk_live_read_bytes.record(self.disk_live_read_bytes, &[]);
        disk_live_write_bytes.record(self.disk_live_write_bytes, &[]);
        processes_live_read_bytes.record(self.processes_live_read_bytes, &[]);
        processes_live_write_bytes.record(self.processes_live_write_bytes, &[]);
        cpu_percent.record(self.cpu_percent, &[]);
        process_memory_bytes.record(self.process_memory_bytes, &[]);
        rss_bytes.record(self.rss_bytes, &[]);
        virtual_bytes.record(self.virtual_bytes, &[]);
        memory_used_bytes.record(self.memory_used_bytes, &[]);
        memory_total_bytes.record(self.memory_total_bytes, &[]);
        open_files.record(self.open_files, &[]);
    }
}

impl From<SystemMetrics> for MetricsEvent {
    fn from(value: SystemMetrics) -> Self {
        MetricsEvent::SystemMetrics(value)
    }
}
