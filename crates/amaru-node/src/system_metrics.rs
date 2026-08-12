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

//! Process and build-identity gauges expected by e2e metric contracts.
//!
//! The product binary and embedder [`crate::Telemetry`] both start this loop so
//! OTLP exports include `process_*` and `cardano_node_metrics_cardano_*`
//! instruments (see `scripts/compare-metrics.json`).

use std::{
    sync::{Arc, LazyLock},
    time::Duration,
};

use amaru_kernel::utils::process::sample_process_block_io_ticks;
#[cfg(unix)]
use amaru_kernel::utils::process::sample_process_memory;
use amaru_metrics::{Meter, MetricRecorder, MetricsEvent, SystemMetrics, initialize_metrics};
use anyhow::anyhow;
use opentelemetry::KeyValue;
use sysinfo::{
    CpuRefreshKind, DiskRefreshKind, Disks, MemoryRefreshKind, Networks, ProcessRefreshKind, ProcessesToUpdate,
    RefreshKind, System,
};
use tokio::task::JoinHandle;
use tracing::error;

static METRICS_POLL_DELAY: LazyLock<Duration> = LazyLock::new(|| Duration::from_secs(1));

/// Identity labels recorded on the Cardano-compatible build-info gauges.
#[derive(Debug, Clone, Copy)]
pub struct BuildIdentity {
    pub version: &'static str,
    pub revision: &'static str,
    pub dirty: bool,
    pub os: &'static str,
    pub arch: &'static str,
}

impl Default for BuildIdentity {
    fn default() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION"),
            revision: "unknown",
            dirty: false,
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
        }
    }
}

#[cfg(unix)]
fn sampled_process_memory_bytes(pid: sysinfo::Pid, rss_bytes: u64) -> u64 {
    sample_process_memory(pid.as_u32()).unwrap_or(rss_bytes)
}

#[cfg(not(unix))]
fn sampled_process_memory_bytes(_pid: sysinfo::Pid, rss_bytes: u64) -> u64 {
    rss_bytes
}

/// Record build-info gauges once, then poll process/host samples into `meter`.
///
/// Returns a join handle for the background poller (always `Some` on success).
/// Callers should abort it on process shutdown.
pub fn track_system_metrics(
    meter: Arc<Meter>,
    build: BuildIdentity,
) -> Result<Option<JoinHandle<()>>, Box<dyn std::error::Error>> {
    record_build_info(&meter, build);
    initialize_metrics(&meter);

    let mut sys = System::new_with_specifics(
        RefreshKind::nothing()
            .with_cpu(CpuRefreshKind::everything().without_frequency())
            .with_memory(MemoryRefreshKind::nothing().with_ram()),
    );
    let mut disks = Disks::new_with_refreshed_list_specifics(DiskRefreshKind::nothing().with_io_usage());
    let mut networks = Networks::new_with_refreshed_list();
    let number_of_cpus = sys.cpus().len() as u64;

    let own_pid = sysinfo::get_current_pid().map_err(|err| anyhow!("unable to retrieve own pid: {err}"))?;

    Ok(Some(tokio::spawn(async move {
        loop {
            tokio::time::sleep(*METRICS_POLL_DELAY).await;

            sys.refresh_processes_specifics(
                ProcessesToUpdate::Some(&[own_pid]),
                true,
                ProcessRefreshKind::nothing().with_cpu().with_disk_usage().with_memory(),
            );
            sys.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());
            disks.refresh_specifics(false, DiskRefreshKind::nothing().with_io_usage());
            networks.refresh(true);

            match sys.process(own_pid) {
                None => error!("unable to find amaru's own process (pid={own_pid}) ?!"),
                Some(process) => {
                    let disk_usage = process.disk_usage();
                    let process_memory_live_resident = process.memory();
                    let (host_live_read_bytes, host_live_write_bytes) =
                        disks.iter().fold((0u64, 0u64), |(read_total, write_total), disk| {
                            let usage = disk.usage();
                            (
                                read_total.saturating_add(usage.read_bytes),
                                write_total.saturating_add(usage.written_bytes),
                            )
                        });
                    let (network_read_bytes, network_written_bytes) =
                        networks.values().fold((0u64, 0u64), |(read_total, write_total), network| {
                            (
                                read_total.saturating_add(network.total_received()),
                                write_total.saturating_add(network.total_transmitted()),
                            )
                        });
                    let event = MetricsEvent::SystemMetrics(SystemMetrics {
                        node_start_time_seconds: process.start_time(),
                        cpu_ticks: process.accumulated_cpu_time() / 10,
                        block_io_ticks: sample_process_block_io_ticks(),
                        network_read_bytes,
                        network_written_bytes,
                        runtime_seconds: process.run_time(),
                        cpu_percent: process.cpu_usage() as f64 / number_of_cpus as f64,
                        process_memory_bytes: sampled_process_memory_bytes(own_pid, process_memory_live_resident),
                        process_memory_live_resident,
                        process_memory_available_virtual: process.virtual_memory(),
                        memory_used_bytes: sys.used_memory(),
                        memory_total_bytes: sys.total_memory(),
                        disk_read_bytes: disk_usage.total_read_bytes,
                        disk_write_bytes: disk_usage.total_written_bytes,
                        disk_live_read_bytes: disk_usage.read_bytes,
                        disk_live_write_bytes: disk_usage.written_bytes,
                        host_live_read_bytes,
                        host_live_write_bytes,
                        open_files: process.open_files().map_or(0, |files| files as u64),
                    });

                    event.record_to_meter(&meter);
                }
            }
        }
    })))
}

/// Expose cardano-node's replay-progress metric once Amaru's stage graph is running.
///
/// This deliberately reports only the ready state rather than cardano-node's per-slot progress:
/// <https://github.com/IntersectMBO/cardano-node/blob/master/cardano-node/src/Cardano/Node/Tracing/Tracers/BlockReplayProgress.hs>
pub fn record_block_replay_ready(meter: &Meter) {
    let Some(meter) = meter.get() else {
        return;
    };

    meter
        .f64_gauge("cardano_node_metrics_blockReplayProgress_real")
        .with_description("whether the node has completed startup")
        .with_unit("%")
        .build()
        .record(100.0, &[]);
}

fn record_build_info(meter: &Meter, build: BuildIdentity) {
    let Some(meter) = meter.get() else {
        return;
    };

    let build_info = meter
        .u64_gauge("cardano_node_metrics_cardano_build_info")
        .with_description("build information for the running amaru node")
        .build();

    build_info.record(
        1,
        &[
            KeyValue::new("version", build.version),
            KeyValue::new("revision", build.revision),
            KeyValue::new("dirty", build.dirty.to_string()),
            KeyValue::new("os", build.os),
            KeyValue::new("arch", build.arch),
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

    let version_parts: Vec<&str> = build.version.split('.').collect();

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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use amaru_metrics::{METRICS_METER_NAME, Meter};
    use opentelemetry::metrics::MeterProvider as _;
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};

    use super::*;

    #[tokio::test]
    async fn track_system_metrics_emits_build_and_process_gauges() {
        let exporter = InMemoryMetricExporter::default();
        let reader = PeriodicReader::builder(exporter.clone()).with_interval(Duration::from_millis(200)).build();
        let provider = SdkMeterProvider::builder().with_reader(reader).build();
        let meter = Arc::new(Meter::from(provider.meter(METRICS_METER_NAME)));

        let handle = track_system_metrics(
            Arc::clone(&meter),
            BuildIdentity { version: "10.11.0", revision: "deadbeef", dirty: false, os: "linux", arch: "x86_64" },
        )
        .expect("start system metrics")
        .expect("poller handle");
        record_block_replay_ready(&meter);

        // Wait for process poll + periodic export.
        tokio::time::sleep(Duration::from_millis(1500)).await;
        provider.force_flush().expect("flush metrics");
        handle.abort();

        let exported = exporter.get_finished_metrics().expect("finished metrics");
        let names: std::collections::BTreeSet<String> = exported
            .iter()
            .flat_map(|rm| rm.scope_metrics().flat_map(|scope| scope.metrics().map(|m| m.name().to_string())))
            .collect();

        for expected in [
            "cardano_node_metrics_cardano_build_info",
            "cardano_node_metrics_cardano_version_major_int",
            "cardano_node_metrics_cardano_version_minor_int",
            "cardano_node_metrics_cardano_version_patch_int",
            "cardano_node_metrics_blockReplayProgress_real",
            "process_cpu_live",
            "process_disk_live_read",
            "process_disk_live_write",
            "process_disk_total_read",
            "process_disk_total_write",
            "process_memory_available_virtual",
            "process_memory_live_resident",
            "process_open_files",
            "process_runtime",
        ] {
            assert!(names.iter().any(|n| n == expected), "missing metric {expected}; have {names:?}");
        }

        let replay_progress = exported
            .iter()
            .flat_map(|rm| rm.scope_metrics().flat_map(|scope| scope.metrics()))
            .find(|metric| metric.name() == "cardano_node_metrics_blockReplayProgress_real")
            .expect("block replay progress metric");
        let opentelemetry_sdk::metrics::data::AggregatedMetrics::F64(
            opentelemetry_sdk::metrics::data::MetricData::Gauge(replay_progress),
        ) = replay_progress.data()
        else {
            panic!("block replay progress should be an f64 gauge");
        };
        assert_eq!(replay_progress.data_points().next().expect("replay progress data point").value(), 100.0);
    }
}
