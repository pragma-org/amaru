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
use tokio::task::JoinHandle;

pub fn track_system_metrics(metrics: SdkMeterProvider) -> JoinHandle<()> {
    use internals::*;
    use std::time::Duration;

    tokio::spawn(async move {
        let counters = make_system_counters(metrics);
        // TODO(pi): configurable parameter?
        let delay = Duration::from_secs(1);
        loop {
            tokio::time::sleep(delay).await;

            record_system_metrics(&counters);
        }
    })
}

mod internals {
    use opentelemetry::{
        metrics::{Counter, Gauge, MeterProvider},
        KeyValue,
    };
    use opentelemetry_sdk::metrics::SdkMeterProvider;
    use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

    pub struct SystemCounters {
        total_memory: Gauge<u64>,
        free_memory: Gauge<u64>,
        #[cfg(not(windows))]
        cpu_load: Gauge<f64>,
        cpu_usage: Counter<f64>,
    }

    pub fn make_system_counters(metrics: SdkMeterProvider) -> SystemCounters {
        // TODO: standardize with the Haskell node somehow?
        let meter = metrics.meter("system");
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

        #[cfg(not(windows))]
        let cpu_load = meter
            .f64_gauge("cpu.utilization")
            .with_description("the 1m average load, measured once per second")
            .build();

        let cpu_usage = meter
            .f64_counter("cpu.percent")
            .with_description("the cpu usage in percent, measured once per second")
            .with_unit("ms")
            .build();

        SystemCounters {
            total_memory,
            free_memory,
            #[cfg(not(windows))]
            cpu_load,
            cpu_usage,
        }
    }

    pub fn record_system_metrics(counters: &SystemCounters) {
        let sys = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::everything().with_frequency())
                .with_memory(MemoryRefreshKind::everything().without_swap()),
        );

        counters.total_memory.record(sys.total_memory(), &[]);
        counters.free_memory.record(sys.free_memory(), &[]);
        #[cfg(not(windows))]
        counters.cpu_load.record(System::load_average().one, &[]);
        let usages = sys
            .cpus()
            .iter()
            .map(|cpu| (cpu.name().into(), cpu.cpu_usage()))
            .collect::<Vec<(String, f32)>>();
        for cpu in usages {
            counters
                .cpu_usage
                .add(cpu.1 as f64, &[KeyValue::new("cpu_name", cpu.0)]);
        }
    }
}
