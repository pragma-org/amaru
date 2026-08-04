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

use amaru_metrics::{METRICS_METER_NAME, MetricRecorder, SystemMetrics, has_subscribers, notify_subscribers};
use anyhow::anyhow;
use opentelemetry::{KeyValue, metrics::MeterProvider};
use opentelemetry_sdk::metrics::SdkMeterProvider;
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};
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

    let own_pid = sysinfo::get_current_pid().map_err(|err| anyhow!("unable to retrieve own pid: {err}"))?;

    Ok(Some(tokio::spawn(async move {
        loop {
            tokio::time::sleep(*METRICS_POLL_DELAY).await;

            sys.refresh_processes_specifics(
                ProcessesToUpdate::Some(&[own_pid]),
                true,
                ProcessRefreshKind::nothing().with_cpu().with_disk_usage().with_memory(),
            );

            match sys.process(own_pid) {
                None => error!("unable to find amaru's own process (pid={own_pid}) ?!"),
                Some(process) => {
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

                    if let Some(meter) = meter.as_ref() {
                        metrics.record_to_meter(meter);
                    }
                    notify_subscribers(&metrics.into());
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
