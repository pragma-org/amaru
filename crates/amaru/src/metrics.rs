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

use amaru_metrics::{
    METRICS_METER_NAME, MetricRecorder, SystemMetrics, has_subscribers, initialize_metrics, notify_subscribers,
};
use anyhow::anyhow;
use opentelemetry::{
    KeyValue,
    metrics::{Gauge, Meter, MeterProvider},
};
use opentelemetry_sdk::metrics::SdkMeterProvider;
use sysinfo::{
    CpuRefreshKind, MemoryRefreshKind, Networks, Process, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System,
};
use tokio::task::JoinHandle;
use tracing::error;

use crate::version;

static METRICS_POLL_DELAY: LazyLock<Duration> = LazyLock::new(|| Duration::from_secs(1));

pub fn track_system_metrics(
    provider: Option<SdkMeterProvider>,
) -> Result<Option<JoinHandle<()>>, Box<dyn std::error::Error>> {
    if provider.is_none() && !has_subscribers() {
        return Ok(None);
    }

    if let Some(provider) = provider.as_ref() {
        record_build_info(provider);
    }

    let mut sys = System::new_with_specifics(
        RefreshKind::nothing()
            .with_cpu(CpuRefreshKind::everything().without_frequency())
            .with_memory(MemoryRefreshKind::everything().without_swap()),
    );
    let number_of_cpus = sys.cpus().len() as u64;
    let meter = provider.as_ref().map(|provider| provider.meter(METRICS_METER_NAME));
    if let Some(meter) = meter.as_ref() {
        initialize_metrics(meter);
    }

    let own_pid = sysinfo::get_current_pid().map_err(|err| anyhow!("unable to retrieve own pid: {err}"))?;
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[own_pid]),
        true,
        ProcessRefreshKind::nothing().with_cpu().with_disk_usage().with_memory(),
    );

    let process = sys.process(own_pid).ok_or_else(|| anyhow!("unable to find amaru's own process (pid={own_pid})"))?;
    let mut networks = Networks::new_with_refreshed_list();
    let cardano_metrics = meter.as_ref().map(|meter| CardanoMetrics::new(meter, process.start_time()));
    record_system_metrics(process, &networks, number_of_cpus, meter.as_ref(), cardano_metrics.as_ref());

    Ok(Some(tokio::spawn(async move {
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
                    record_system_metrics(process, &networks, number_of_cpus, meter.as_ref(), cardano_metrics.as_ref());
                }
            }
        }
    })))
}

fn record_build_info(provider: &SdkMeterProvider) {
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

fn record_system_metrics(
    process: &Process,
    networks: &Networks,
    number_of_cpus: u64,
    meter: Option<&Meter>,
    cardano_metrics: Option<&CardanoMetrics>,
) {
    let disk_usage = process.disk_usage();
    let metrics = SystemMetrics {
        runtime_seconds: process.run_time(),
        cpu_percent: process.cpu_usage() as f64 / number_of_cpus as f64,
        process_memory_bytes: process.memory(),
        rss_bytes: process.memory(),
        virtual_bytes: process.virtual_memory(),
        disk_read_bytes: disk_usage.total_read_bytes,
        disk_write_bytes: disk_usage.total_written_bytes,
        disk_live_read_bytes: disk_usage.read_bytes,
        disk_live_write_bytes: disk_usage.written_bytes,
        open_files: process.open_files().map_or(0, |files| files as u64),
    };

    if let Some(meter) = meter {
        metrics.record_to_meter(meter);
    }
    if let Some(cardano_metrics) = cardano_metrics {
        cardano_metrics.record(process, networks);
    }
    notify_subscribers(&metrics.into());
}

struct CardanoMetrics {
    cpu_ticks: Gauge<u64>,
    network_read_bytes: Gauge<u64>,
    network_written_bytes: Gauge<u64>,
    filesystem_read_bytes: Gauge<u64>,
    filesystem_written_bytes: Gauge<u64>,
    memory_resident_bytes: Gauge<u64>,
}

impl CardanoMetrics {
    fn new(meter: &Meter, node_start_time_seconds: u64) -> Self {
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

        Self {
            cpu_ticks: meter
                .u64_gauge("cardano_node_metrics_Stat_cputicks_int")
                .with_description("total CPU time used by the process in centiseconds")
                .with_unit("centiseconds")
                .build(),
            network_read_bytes: meter
                .u64_gauge("cardano_node_metrics_Stat_netRd_int")
                .with_description("total bytes received by the host's network interfaces")
                .with_unit("bytes")
                .build(),
            network_written_bytes: meter
                .u64_gauge("cardano_node_metrics_Stat_netWr_int")
                .with_description("total bytes sent by the host's network interfaces")
                .with_unit("bytes")
                .build(),
            filesystem_read_bytes: meter
                .u64_gauge("cardano_node_metrics_Stat_fsRd_int")
                .with_description("total bytes read from storage by the process")
                .with_unit("bytes")
                .build(),
            filesystem_written_bytes: meter
                .u64_gauge("cardano_node_metrics_Stat_fsWr_int")
                .with_description("total bytes written to storage by the process")
                .with_unit("bytes")
                .build(),
            memory_resident_bytes: meter
                .u64_gauge("cardano_node_metrics_Mem_resident_int")
                .with_description("kernel-reported resident set size")
                .with_unit("bytes")
                .build(),
        }
    }

    fn record(&self, process: &Process, networks: &Networks) {
        self.cpu_ticks.record(process.accumulated_cpu_time() / 10, &[]);

        let (network_read_bytes, network_written_bytes) = networks.values().fold((0u64, 0u64), |totals, network| {
            (totals.0.saturating_add(network.total_received()), totals.1.saturating_add(network.total_transmitted()))
        });
        self.network_read_bytes.record(network_read_bytes, &[]);
        self.network_written_bytes.record(network_written_bytes, &[]);

        let disk_usage = process.disk_usage();
        self.filesystem_read_bytes.record(disk_usage.total_read_bytes, &[]);
        self.filesystem_written_bytes.record(disk_usage.total_written_bytes, &[]);
        self.memory_resident_bytes.record(process.memory(), &[]);
    }
}
