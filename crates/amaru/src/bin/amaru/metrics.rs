use std::time::Duration;

use miette::IntoDiagnostic;
use opentelemetry::{
    metrics::{Counter, Gauge, MeterProvider},
    KeyValue,
};
use opentelemetry_sdk::metrics::SdkMeterProvider;
use sys_metrics::{
    cpu::{CpuTimes, LoadAvg},
    memory::Memory,
};
use tokio::task::JoinHandle;
use tracing::warn;

#[cfg(not(windows))]
pub fn track_system_metrics(metrics: SdkMeterProvider) -> JoinHandle<()> {
    tokio::spawn(async move {
        let counters = make_system_counters(metrics);
        let mut delay = Duration::from_secs(1);
        loop {
            // TODO(pi): configurable parameter?
            tokio::time::sleep(delay).await;

            let reading = match get_reading() {
                Ok(sys) => sys,
                Err(err) => {
                    warn!("failed to read system metrics: {}", err);
                    // Back off slightly so the logs aren't as noisy
                    delay *= 2;
                    if delay > Duration::from_secs(30) {
                        delay = Duration::from_secs(30);
                    }
                    continue;
                }
            };
            delay = Duration::from_secs(1);

            record_system_metrics(reading, &counters);
        }
    })
}

#[cfg(windows)]
pub fn track_system_metrics(metrics: SdkMeterProvider) -> JoinHandle<()> {
    use tracing::info;
    info!("System metrics currently not supported on Windows");
    tokio::spawn(async {})
}

struct SystemCounters {
    total_memory: Gauge<u64>,
    free_memory: Gauge<u64>,
    cpu_load: Gauge<f64>,
    user_time: Counter<u64>,
}

#[derive(Debug)]
struct Reading {
    memory: Memory,
    cpu: CpuTimes,
    load: LoadAvg,
}

fn make_system_counters(metrics: SdkMeterProvider) -> SystemCounters {
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

    let cpu_load = meter
        .f64_gauge("cpu.utilization")
        .with_description("the 1m average load, measured once per second")
        .build();

    let user_time = meter
        .u64_counter("cpu.time")
        .with_description("the total cpu time spent in user processes")
        .with_unit("ms")
        .build();

    SystemCounters {
        total_memory,
        free_memory,
        cpu_load,
        user_time,
    }
}

#[cfg(not(windows))]
fn get_reading() -> miette::Result<Reading> {
    use sys_metrics::*;
    let memory = memory::get_memory().into_diagnostic()?;
    let cpu = cpu::get_cputimes().into_diagnostic()?;
    let load = cpu::get_loadavg().into_diagnostic()?;

    Ok(Reading { memory, cpu, load })
}

fn record_system_metrics(reading: Reading, counters: &SystemCounters) {
    counters.total_memory.record(reading.memory.total, &[]);
    counters.free_memory.record(reading.memory.free, &[]);
    counters.cpu_load.record(reading.load.one, &[]);
    counters
        .user_time
        .add(reading.cpu.user, &[KeyValue::new("state", "user")]);
    counters
        .user_time
        .add(reading.cpu.system, &[KeyValue::new("state", "system")]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_read_system_metrics() {
        let reading = get_reading().expect("failed to read system metrics");
        assert!(reading.memory.free > 0, "failed to read free memory");
        assert!(reading.memory.total > 0, "failed to read total memory");
        assert!(reading.cpu.user > 0, "failed to read user cpu time");
        assert!(reading.load.one > 0.0, "failed to read cpu load average");
    }
}
