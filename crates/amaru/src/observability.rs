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

//! Product defaults and CLI adapters for the shared node telemetry installer.

use std::{env::var, io::IsTerminal, str::FromStr};

use amaru_node::telemetry::{
    LocalTelemetry, LogFormat, LogOutput, OtelSignal, OtelSignals, OtlpConfig, StartupNotice, TelemetryConfig,
    TelemetryHandle, TraceOutputSpec, filter_from_env, resource,
};
use amaru_observability::{
    info,
    tracing_subscriber::{
        EnvFilter,
        fmt::{format::FmtSpan, writer::BoxMakeWriter},
    },
};
use amaru_progress_bar::{ProgressLogWriter, progress_log_writer};
use opentelemetry_sdk::Resource;

const DEFAULT_AMARU_LOG_FILTER: &str = "error,amaru=info,amaru_pure_stage=warn";
const DEFAULT_AMARU_TRACE_FILTER: &str = "error,amaru=debug,amaru_pure_stage=warn";

#[cfg(test)]
mod tests;

/// Context hints supplied by the product binary to refine observability defaults.
pub trait ObservabilityHints {
    /// The node's listen address, used as a fallback for `service.instance.id`.
    fn listen_address(&self) -> Option<&str>;
}

pub fn new_resource(hints: &impl ObservabilityHints) -> Resource {
    let instance_id = var("OTEL_SERVICE_INSTANCE_ID")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            let address = hints.listen_address()?;
            let hostname = sysinfo::System::host_name().unwrap_or_else(|| "localhost".to_string());
            let port = address.trim().rsplit(':').next()?;
            Some(format!("{hostname}:{port}"))
        });
    resource(instance_id)
}

pub fn try_setup_observability(
    with_open_telemetry: bool,
    open_telemetry_signals: OtelSignals,
    with_json_traces: bool,
    local: Option<LocalTelemetry>,
    color: bool,
    hints: &impl ObservabilityHints,
    trace_outputs: &[TraceOutputSpec],
) -> anyhow::Result<TelemetryHandle> {
    try_setup_observability_reporting(
        with_open_telemetry,
        open_telemetry_signals,
        with_json_traces,
        local,
        color,
        hints,
        trace_outputs,
        true,
    )
}

/// Configure observability while optionally suppressing startup configuration events.
#[expect(clippy::too_many_arguments)]
pub fn try_setup_observability_reporting(
    with_open_telemetry: bool,
    open_telemetry_signals: OtelSignals,
    with_json_traces: bool,
    local: Option<LocalTelemetry>,
    color: bool,
    hints: &impl ObservabilityHints,
    trace_outputs: &[TraceOutputSpec],
    report_setup_logs: bool,
) -> anyhow::Result<TelemetryHandle> {
    let (config, notices) = observability_config(
        with_open_telemetry,
        open_telemetry_signals,
        with_json_traces,
        local,
        color,
        hints,
        trace_outputs,
    );
    let handle = config.install()?;
    if report_setup_logs {
        for notice in notices {
            notice();
        }
        info!(setup::observability::INIT, with_open_telemetry, with_json_traces, with_colors = color);
    }
    Ok(handle)
}

/// Resolve CLI telemetry settings without installing a subscriber.
/// Startup notices must be emitted only after installation succeeds.
fn observability_config(
    with_open_telemetry: bool,
    open_telemetry_signals: OtelSignals,
    with_json_traces: bool,
    local: Option<LocalTelemetry>,
    color: bool,
    hints: &impl ObservabilityHints,
    trace_outputs: &[TraceOutputSpec],
) -> (TelemetryConfig, Vec<StartupNotice>) {
    let mut notices = Vec::new();
    let mut trace_filter = || {
        let (filter, notice) = filter_from_env("AMARU_TRACE", DEFAULT_AMARU_TRACE_FILTER);
        notices.push(notice);
        filter
    };
    let otlp = with_open_telemetry.then(|| {
        let filter = if open_telemetry_signals.0.contains(&OtelSignal::Traces)
            || open_telemetry_signals.0.contains(&OtelSignal::Logs)
        {
            trace_filter()
        } else {
            EnvFilter::new("off")
        };
        OtlpConfig { resource: new_resource(hints), signals: open_telemetry_signals, filter }
    });
    let local = local.unwrap_or_default();
    let capture_filter = if local.capture_layer.is_some() { trace_filter() } else { EnvFilter::new("off") };
    let output = if with_json_traces {
        Some(LogOutput {
            format: LogFormat::Json,
            writer: BoxMakeWriter::new(std::io::stdout),
            filter: trace_filter(),
            span_events: FmtSpan::ACTIVE | FmtSpan::CLOSE,
        })
    } else if local.capture_layer.is_some() {
        None
    } else {
        let (filter, notice) = filter_from_env("AMARU_LOG", DEFAULT_AMARU_LOG_FILTER);
        notices.push(notice);
        Some(LogOutput {
            format: if color { LogFormat::Ansi } else { LogFormat::Plain },
            writer: BoxMakeWriter::new(progress_log_writer as fn() -> ProgressLogWriter),
            filter,
            span_events: FmtSpan::NEW | FmtSpan::CLOSE,
        })
    };
    let config = TelemetryConfig {
        otlp,
        output,
        local,
        capture_filter,
        files: trace_outputs.to_vec(),
        report_export_errors: true,
        ..TelemetryConfig::default()
    };
    (config, notices)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Never,
    Always,
    Auto,
}
impl FromStr for Color {
    type Err = &'static str;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "off" | "never" => Ok(Color::Never),
            "on" | "always" => Ok(Color::Always),
            "auto" => Ok(Color::Auto),
            _ => Err("valid color settings are 'on', 'always', 'off', 'never' or 'auto'"),
        }
    }
}
impl Color {
    pub fn is_enabled(this: Self) -> bool {
        match this {
            Color::Never => false,
            Color::Always => true,
            Color::Auto => {
                if std::env::var("NO_COLOR").iter().any(|s| !s.is_empty()) {
                    false
                } else {
                    std::io::stderr().is_terminal()
                }
            }
        }
    }
}
