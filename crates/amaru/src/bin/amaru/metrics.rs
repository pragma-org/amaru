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

use opentelemetry_sdk::metrics::SdkMeterProvider;
use std::{sync::LazyLock, time::Duration};
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};
use tokio::task::JoinHandle;
use tracing::error;

static METRICS_POLL_DELAY: LazyLock<Duration> = LazyLock::new(|| Duration::from_secs(1));

pub fn track_system_metrics(provider: SdkMeterProvider) -> JoinHandle<()> {
    use internals::*;
    tokio::spawn(async move {
        let metrics = ProcessMetrics::new(provider);
        let mut sys = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::everything().without_frequency())
                .with_memory(MemoryRefreshKind::everything().without_swap()),
        );
        loop {
            tokio::time::sleep(*METRICS_POLL_DELAY).await;
            sys.refresh_cpu_all();
            metrics
                .record(&sys)
                .unwrap_or_else(|err| error!("failed polling metrics: {err}"))
        }
    })
}

mod internals {
    use opentelemetry::{
        metrics::{Gauge, MeterProvider},
        KeyValue,
    };
    use opentelemetry_sdk::metrics::SdkMeterProvider;
    use sysinfo::System;

    pub struct ProcessMetrics {
        total_memory: Gauge<u64>,
        free_memory: Gauge<u64>,
        cpu_usage: Gauge<f64>,
    }

    impl ProcessMetrics {
        pub fn new(metrics: SdkMeterProvider) -> Self {
            // TODO: standardize with the Haskell node somehow?
            let meter = metrics.meter("amaru");

            let total_memory = meter
                .u64_gauge("memory.limit")
                .with_description("The total system memory, updated once per second")
                .with_unit("MB")
                .build();

            let free_memory = meter
                .u64_gauge("memory.usage")
                .with_description("The free system memory, measured once per second")
                .with_unit("MB")
                .build();

            let cpu_usage = meter
                .f64_gauge("cpu.percent")
                .with_description("the cpu usage in percent, measured once per second")
                .with_unit("%")
                .build();

            Self {
                total_memory,
                free_memory,
                cpu_usage,
            }
        }

        pub fn record(&self, sys: &System) -> Result<(), Box<dyn std::error::Error>> {
            self.total_memory.record(sys.total_memory(), &[]);

            self.free_memory.record(sys.free_memory(), &[]);

            let usages = sys
                .cpus()
                .iter()
                .map(|cpu| (cpu.name().into(), cpu.cpu_usage()))
                .collect::<Vec<(String, f32)>>();

            for cpu in usages {
                self.cpu_usage
                    .record(cpu.1 as f64, &[KeyValue::new("cpu_name", cpu.0)]);
            }

            Ok(())
        }
    }
}
