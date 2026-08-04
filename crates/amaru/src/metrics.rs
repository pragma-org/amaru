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

use std::{sync::LazyLock, time::Duration};

use amaru_metrics::METRICS_METER_NAME;
use anyhow::anyhow;
use opentelemetry::KeyValue;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use sysinfo::{
    CpuRefreshKind, MemoryRefreshKind, Networks, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System,
};
use tokio::task::JoinHandle;
use tracing::error;

use crate::version;

static METRICS_POLL_DELAY: LazyLock<Duration> = LazyLock::new(|| Duration::from_secs(1));

pub fn track_system_metrics(provider: SdkMeterProvider) -> Result<JoinHandle<()>, Box<dyn std::error::Error>> {
    use internals::*;

    record_build_info(&provider);

    let mut sys = System::new_with_specifics(
        RefreshKind::nothing()
            .with_cpu(CpuRefreshKind::everything().without_frequency())
            .with_memory(MemoryRefreshKind::everything().without_swap()),
    );

    let own_pid = sysinfo::get_current_pid().map_err(|err| anyhow!("unable to retrieve own pid: {err}"))?;
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[own_pid]),
        true,
        ProcessRefreshKind::nothing().with_cpu().with_disk_usage().with_memory(),
    );

    let process = sys.process(own_pid).ok_or_else(|| anyhow!("unable to find amaru's own process (pid={own_pid})"))?;
    let mut networks = Networks::new_with_refreshed_list();
    let metrics = ProcessMetrics::new(provider, &sys, process.start_time());
    metrics.record(process, &networks);

    Ok(tokio::spawn(async move {
        loop {
            tokio::time::sleep(*METRICS_POLL_DELAY).await;

            sys.refresh_processes_specifics(
                ProcessesToUpdate::Some(&[own_pid]),
                true,
                ProcessRefreshKind::nothing().with_cpu().with_disk_usage().with_memory(),
            );
            networks.refresh(true);

            match sys.process(own_pid) {
                None => error!("unable to find amaru's own process (pid={own_pid}) ?!"),
                Some(process) => {
                    metrics.record(process, &networks);
                }
            }
        }
    }))
}

fn record_build_info(provider: &SdkMeterProvider) {
    use opentelemetry::metrics::MeterProvider;

    let meter = provider.meter(METRICS_METER_NAME);

    let build_info = meter
        .u64_gauge("cardano_node_metrics_cardano_build_info")
        .with_description("build information for the running amaru node")
        .build();

    build_info.record(
        1,
        &[
            KeyValue::new("version", version::package_version()),
            KeyValue::new("revision", version::git_commit_hash_short().unwrap_or("unknown")),
            KeyValue::new("dirty", version::git_dirty().unwrap_or(false).to_string()),
            KeyValue::new("os", version::target_os()),
            KeyValue::new("arch", version::target_arch()),
        ],
    );

    let forging_enabled = meter
        .u64_gauge("cardano_node_metrics_forging_enabled_int")
        .with_description("whether block forging is enabled")
        .with_unit("int")
        .build();

    forging_enabled.record(0, &[]);

    let version_major = meter
        .u64_gauge("cardano_node_metrics_cardano_version_major_int")
        .with_description("Major version number")
        .build();

    let version_minor = meter
        .u64_gauge("cardano_node_metrics_cardano_version_minor_int")
        .with_description("Minor version number")
        .build();

    let version_patch = meter
        .u64_gauge("cardano_node_metrics_cardano_version_patch_int")
        .with_description("Patch version number")
        .build();

    let version_parts: Vec<&str> = version::package_version().split('.').collect();

    if let Some(major) = version_parts.first().and_then(|v| v.split('-').next()?.parse::<u64>().ok()) {
        version_major.record(major, &[]);
    }
    if let Some(minor) = version_parts.get(1).and_then(|v| v.split('-').next()?.parse::<u64>().ok()) {
        version_minor.record(minor, &[]);
    }
    if let Some(patch) = version_parts.get(2).and_then(|v| v.split('-').next()?.parse::<u64>().ok()) {
        version_patch.record(patch, &[]);
    }
}

mod internals {
    use opentelemetry::{
        KeyValue,
        metrics::{Gauge, MeterProvider},
    };
    use opentelemetry_sdk::metrics::SdkMeterProvider;
    use sysinfo::{Networks, Process, System};

    pub struct ProcessMetrics {
        number_of_cpus: u64,

        runtime_seconds: Gauge<u64>,

        disk_total_read_bytes: Gauge<u64>,
        disk_total_write_bytes: Gauge<u64>,

        disk_live_read_bytes: Gauge<u64>,
        disk_live_write_bytes: Gauge<u64>,

        cpu_live_percent: Gauge<f64>,
        cpu_ticks: Gauge<u64>,
        network_read_bytes: Gauge<u64>,
        network_written_bytes: Gauge<u64>,
        filesystem_read_bytes: Gauge<u64>,
        filesystem_written_bytes: Gauge<u64>,

        memory_live_resident_bytes: Gauge<u64>,
        memory_resident_bytes: Gauge<u64>,

        memory_available_virtual_bytes: Gauge<u64>,

        open_files: Gauge<u64>,
    }

    impl ProcessMetrics {
        pub fn new(metrics: SdkMeterProvider, sys: &System, node_start_time_seconds: u64) -> Self {
            let meter = metrics.meter("amaru");

            let number_of_cpus = sys.cpus().len() as u64;

            let runtime_seconds = meter
                .u64_gauge("process_runtime")
                .with_description("How much time the process has been running (in seconds)")
                .with_unit("seconds")
                .build();

            let node_start_time = meter
                .u64_gauge("cardano_node_metrics_node_start_time_int")
                .with_description("node start time as seconds since the Unix epoch")
                .with_unit("seconds")
                .build();

            let basic_info = meter
                .u64_gauge("cardano_node_metrics_basicInfo")
                .with_description("basic information for the running Amaru node")
                .build();

            node_start_time.record(node_start_time_seconds, &[]);
            basic_info.record(1, &[KeyValue::new("nodeStartTime", node_start_time_seconds.to_string())]);

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

            let cpu_ticks = meter
                .u64_gauge("cardano_node_metrics_Stat_cputicks_int")
                .with_description("total CPU time used by the process in centiseconds")
                .with_unit("centiseconds")
                .build();

            let network_read_bytes = meter
                .u64_gauge("cardano_node_metrics_Stat_netRd_int")
                .with_description("total bytes received by the host's network interfaces")
                .with_unit("bytes")
                .build();

            let network_written_bytes = meter
                .u64_gauge("cardano_node_metrics_Stat_netWr_int")
                .with_description("total bytes sent by the host's network interfaces")
                .with_unit("bytes")
                .build();

            let filesystem_read_bytes = meter
                .u64_gauge("cardano_node_metrics_Stat_fsRd_int")
                .with_description("total bytes read from storage by the process")
                .with_unit("bytes")
                .build();

            let filesystem_written_bytes = meter
                .u64_gauge("cardano_node_metrics_Stat_fsWr_int")
                .with_description("total bytes written to storage by the process")
                .with_unit("bytes")
                .build();

            let memory_live_resident_bytes = meter
                .u64_gauge("process_memory_live_resident")
                .with_description(
                    "The amount of memory that the process allocated and which is currently mapped in physical RAM (in bytes).",
                )
                .with_unit("bytes")
                .build();

            let memory_resident_bytes = meter
                .u64_gauge("cardano_node_metrics_Mem_resident_int")
                .with_description("kernel-reported resident set size")
                .with_unit("bytes")
                .build();

            let memory_available_virtual_bytes = meter
                .u64_gauge("process_memory_available_virtual")
                .with_description(
                    "The amount of memory that the process can access, whether it is currently mapped in physical RAM or not (in bytes).",
                )
                .with_unit("bytes")
                .build();

            let open_files =
                meter.u64_gauge("process_open_files").with_description("Total number of file descriptors.").build();

            Self {
                number_of_cpus,
                runtime_seconds,
                disk_total_read_bytes,
                disk_total_write_bytes,
                disk_live_read_bytes,
                disk_live_write_bytes,
                cpu_live_percent,
                cpu_ticks,
                network_read_bytes,
                network_written_bytes,
                filesystem_read_bytes,
                filesystem_written_bytes,
                memory_live_resident_bytes,
                memory_resident_bytes,
                memory_available_virtual_bytes,
                open_files,
            }
        }

        pub fn record(&self, proc: &Process, networks: &Networks) {
            self.runtime_seconds.record(proc.run_time(), &[]);

            let disk_usage = proc.disk_usage();
            self.disk_total_read_bytes.record(disk_usage.total_read_bytes, &[]);
            self.disk_total_write_bytes.record(disk_usage.total_written_bytes, &[]);
            self.disk_live_read_bytes.record(disk_usage.read_bytes, &[]);
            self.disk_live_write_bytes.record(disk_usage.written_bytes, &[]);
            self.filesystem_read_bytes.record(disk_usage.total_read_bytes, &[]);
            self.filesystem_written_bytes.record(disk_usage.total_written_bytes, &[]);

            self.cpu_live_percent.record(proc.cpu_usage() as f64 / self.number_of_cpus as f64, &[]);
            self.cpu_ticks.record(proc.accumulated_cpu_time() / 10, &[]);

            let (network_read_bytes, network_written_bytes) =
                networks.values().fold((0u64, 0u64), |totals, network| {
                    (
                        totals.0.saturating_add(network.total_received()),
                        totals.1.saturating_add(network.total_transmitted()),
                    )
                });
            self.network_read_bytes.record(network_read_bytes, &[]);
            self.network_written_bytes.record(network_written_bytes, &[]);

            self.memory_live_resident_bytes.record(proc.memory(), &[]);
            self.memory_resident_bytes.record(proc.memory(), &[]);

            self.memory_available_virtual_bytes.record(proc.virtual_memory(), &[]);

            self.open_files.record(proc.open_files().map_or(0, |files| files as u64), &[]);
        }
    }
}
