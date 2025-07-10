// Copyright 2024 PRAGMA
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

use anyhow::anyhow;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use std::{sync::LazyLock, time::Duration};
use sysinfo::{
    CpuRefreshKind, MemoryRefreshKind, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System,
};
use tokio::task::JoinHandle;
use tracing::error;

static METRICS_POLL_DELAY: LazyLock<Duration> = LazyLock::new(|| Duration::from_secs(1));

pub fn track_system_metrics(
    provider: SdkMeterProvider,
) -> Result<JoinHandle<()>, Box<dyn std::error::Error>> {
    use internals::*;

    let mut sys = System::new_with_specifics(
        RefreshKind::nothing()
            .with_cpu(CpuRefreshKind::everything().without_frequency())
            .with_memory(MemoryRefreshKind::everything().without_swap()),
    );

    let metrics = ProcessMetrics::new(provider, &sys);

    let own_pid =
        sysinfo::get_current_pid().map_err(|err| anyhow!("unable to retrieve own pid: {err}"))?;

    Ok(tokio::spawn(async move {
        loop {
            tokio::time::sleep(*METRICS_POLL_DELAY).await;

            sys.refresh_processes_specifics(
                ProcessesToUpdate::Some(&[own_pid]),
                true,
                ProcessRefreshKind::nothing()
                    .with_cpu()
                    .with_disk_usage()
                    .with_memory(),
            );

            match sys.process(own_pid) {
                None => error!("unable to find amaru's own process (pid={own_pid}) ?!"),
                Some(process) => {
                    metrics.record(process);
                }
            }
        }
    }))
}

mod internals {
    use opentelemetry::metrics::{Gauge, MeterProvider};
    use opentelemetry_sdk::metrics::SdkMeterProvider;
    use sysinfo::{Process, System};

    pub struct ProcessMetrics {
        /// Number of available CPUs on the machine.
        number_of_cpus: u64,

        /// How much time the process has been running (in seconds).
        runtime_seconds: Gauge<u64>,

        /// Total number of read bytes (in bytes).
        disk_total_read_bytes: Gauge<u64>,
        /// Total number of written bytes (in bytes).
        disk_total_write_bytes: Gauge<u64>,

        /// Number of read bytes since the last refresh (in bytes).
        disk_live_read_bytes: Gauge<u64>,
        /// Number of written bytes since the last refresh (in bytes).
        disk_live_write_bytes: Gauge<u64>,

        /// Current CPU utilization (in %).
        cpu_live_percent: Gauge<f64>,

        /// The amount of memory that the process allocated and which is currently mapped in physical RAM (in bytes).
        ///
        /// It does not include memory that is swapped out, or, in some operating systems, that has
        /// been allocated but never used.
        memory_live_resident_bytes: Gauge<u64>,

        /// The amount of memory that the process can access, whether it is currently mapped in physical RAM or not (in bytes).
        ///
        /// It includes physical RAM, allocated but not used regions, swapped-out regions, and even
        /// memory associated with memory-mapped files.
        memory_available_virtual_bytes: Gauge<u64>,
    }

    impl ProcessMetrics {
        pub fn new(metrics: SdkMeterProvider, sys: &System) -> Self {
            let meter = metrics.meter("amaru");

            let number_of_cpus = sys.cpus().len() as u64;

            let runtime_seconds = meter
                .u64_gauge("process_runtime")
                .with_description("How much time the process has been running (in seconds)")
                .with_unit("seconds")
                .build();

            let disk_total_read_bytes = meter
                .u64_gauge("process_disk_total_read")
                .with_description("Total number of read bytes (in bytes).")
                .with_unit("bytes")
                .build();

            let disk_total_write_bytes = meter
                .u64_gauge("process_disk_total_write")
                .with_description("Total number of written bytes (in bytes).")
                .with_unit("bytes")
                .build();

            let disk_live_read_bytes = meter
                .u64_gauge("process_disk_live_read")
                .with_description("Number of read bytes since the last refresh (in bytes)")
                .with_unit("bytes")
                .build();

            let disk_live_write_bytes = meter
                .u64_gauge("process_disk_live_write")
                .with_description("Number of written bytes since the last refresh (in bytes)")
                .with_unit("bytes")
                .build();

            let cpu_live_percent = meter
                .f64_gauge("process_cpu_live")
                .with_description("Current CPU utilization (in %)")
                .with_unit("%")
                .build();

            let memory_live_resident_bytes = meter
                .u64_gauge("process_memory_live_resident")
                .with_description("The amount of memory that the process allocated and which is currently mapped in physical RAM (in bytes).")
                .with_unit("bytes")
                .build();

            let memory_available_virtual_bytes = meter
                .u64_gauge("process_memory_available_virtual")
                .with_description("The amount of memory that the process can access, whether it is currently mapped in physical RAM or not (in bytes).")
                .with_unit("bytes")
                .build();

            Self {
                number_of_cpus,
                runtime_seconds,
                disk_total_read_bytes,
                disk_total_write_bytes,
                disk_live_read_bytes,
                disk_live_write_bytes,
                cpu_live_percent,
                memory_live_resident_bytes,
                memory_available_virtual_bytes,
            }
        }

        pub fn record(&self, proc: &Process) {
            self.runtime_seconds.record(proc.run_time(), &[]);

            let disk_usage = proc.disk_usage();
            self.disk_total_read_bytes
                .record(disk_usage.total_read_bytes, &[]);
            self.disk_total_write_bytes
                .record(disk_usage.total_written_bytes, &[]);
            self.disk_live_read_bytes.record(disk_usage.read_bytes, &[]);
            self.disk_live_write_bytes
                .record(disk_usage.written_bytes, &[]);

            self.cpu_live_percent
                .record(proc.cpu_usage() as f64 / self.number_of_cpus as f64, &[]);

            self.memory_live_resident_bytes.record(proc.memory(), &[]);

            self.memory_available_virtual_bytes
                .record(proc.virtual_memory(), &[]);
        }
    }
}
