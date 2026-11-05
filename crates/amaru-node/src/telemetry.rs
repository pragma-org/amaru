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

//! Shared telemetry setup for the product CLI and embedding applications.
//!
//! [`TelemetryConfig`] builds the subscriber from explicit filters, outputs and OTLP
//! settings. [`Telemetry::install`] supplies the embedder environment defaults and
//! optionally collects process metrics. The CLI supplies its own defaults and TUI
//! capture layer to the same installer.
//!
//! [`TelemetryGuard`] owns exporter shutdown. Keep the Tokio runtime alive until
//! shutdown completes; OTLP exporters schedule work on that runtime.

use std::{env, sync::Arc};

use amaru_metrics::Meter;
use amaru_progress_bar::{ProgressLogWriter, progress_log_writer};
use anyhow::Context;
use opentelemetry::KeyValue;
use opentelemetry_sdk::Resource;
use opentelemetry_semantic_conventions::resource::{SERVICE_INSTANCE_ID, SERVICE_NAME};
use tracing_subscriber::{
    EnvFilter,
    fmt::{format::FmtSpan, writer::BoxMakeWriter},
};

mod file;
mod filter;
mod otlp;
mod subscriber;

pub use file::TraceOutputSpec;
pub use filter::{StartupNotice, filter_from_env};
#[cfg(test)]
pub(crate) use otlp::NotingSpanProcessor;
pub use otlp::{
    BuildOpenTelemetryProvidersError, CborSpanExporter, DEFAULT_METRIC_EXPORT_INTERVAL, OTEL_METRIC_EXPORT_INTERVAL,
    OpenTelemetryProviders, OtelSignal, OtelSignals, ShutdownOpenTelemetryProvidersError,
};
pub use subscriber::{
    ExistingSubscriber, LocalTelemetry, LogOutput, OtlpConfig, TelemetryConfig, TelemetryGuard, TelemetryHandle,
};

const DEFAULT_SERVICE_NAME: &str = "amaru";
const DEFAULT_AMARU_TRACE: &str = "info";

/// How the local tracing subscriber writes events.
///
/// JSON is exclusive with console colour: [`Self::Json`] emits NDJSON on stdout,
/// while [`Self::Plain`] and [`Self::Ansi`] write compact human-readable lines
/// on stderr.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogFormat {
    /// Compact console on stderr, no ANSI colour.
    #[default]
    Plain,
    /// Compact console on stderr with ANSI colour.
    Ansi,
    /// JSON log lines on stdout (trace-contract / machine consumers).
    Json,
}

/// Optional behavior for [`Telemetry::install_with_options`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryOptions {
    /// Whether OTLP telemetry starts the sysinfo-backed process and host metrics poller.
    pub collect_system_metrics: bool,
    /// OpenTelemetry signals to export when OTLP is enabled.
    pub open_telemetry_signals: OtelSignals,
}

impl Default for TelemetryOptions {
    fn default() -> Self {
        Self { collect_system_metrics: true, open_telemetry_signals: OtelSignals::default() }
    }
}

impl LogFormat {
    fn is_json(self) -> bool {
        matches!(self, Self::Json)
    }

    fn ansi(self) -> bool {
        matches!(self, Self::Ansi)
    }
}

/// Active telemetry pipeline for an embedder process.
pub struct Telemetry {
    pub meter: Arc<Meter>,
    teardown: TelemetryGuard,
}

impl Telemetry {
    /// Install process-wide tracing and optional OTLP export using embedder defaults.
    ///
    /// Environment:
    /// - `AMARU_WITH_OPEN_TELEMETRY` — when truthy (`1`/`true`/`yes`/`on`), enable OTLP.
    /// - `AMARU_WITH_JSON_TRACES` — when truthy, force JSON output on stdout.
    /// - `AMARU_TRACE` — filter for exported traces/logs and local output (default `info`).
    /// - `RUST_LOG` — fallback filter for local output when `AMARU_TRACE` is absent.
    /// - `OTEL_SERVICE_NAME` — resource service name (default `amaru`).
    /// - `OTEL_METRIC_EXPORT_INTERVAL` — milliseconds between two OTLP metrics
    ///   exports (default `1000`).
    ///
    /// Must be polled on a Tokio runtime. Use [`TelemetryConfig`] for explicit configuration.
    pub async fn install(format: LogFormat) -> anyhow::Result<Self> {
        Self::install_with_options(format, TelemetryOptions::default()).await
    }

    /// Install telemetry with explicit control over optional collectors and OTLP signals.
    ///
    /// Set [`TelemetryOptions::collect_system_metrics`] to `false` when sysinfo
    /// collection is unavailable, including on iOS.
    pub async fn install_with_options(format: LogFormat, options: TelemetryOptions) -> anyhow::Result<Self> {
        let with_otlp = env_flag("AMARU_WITH_OPEN_TELEMETRY");
        let format = if env_flag("AMARU_WITH_JSON_TRACES") { LogFormat::Json } else { format };
        tokio::task::yield_now().await;
        let collect = with_otlp
            && options.collect_system_metrics
            && options.open_telemetry_signals.0.contains(&OtelSignal::Metrics);
        let config = embedder_config(format, with_otlp.then_some(options.open_telemetry_signals))?;
        let mut telemetry = Self::from_handle(config.install()?);
        if collect {
            telemetry.teardown.collect_system_metrics(Arc::clone(&telemetry.meter))?;
        }
        Ok(telemetry)
    }

    /// Install console or JSON output without OTLP export or system-metric collection.
    pub fn install_local(format: LogFormat) -> anyhow::Result<Self> {
        Ok(Self::from_handle(embedder_config(format, None)?.install()?))
    }

    fn from_handle(handle: TelemetryHandle) -> Self {
        Self { meter: Arc::new(handle.meter), teardown: handle.teardown }
    }

    /// Stop collection and flush exporters before stopping the Tokio runtime.
    pub async fn shutdown(self) -> anyhow::Result<()> {
        tokio::task::yield_now().await;
        self.teardown.shutdown()
    }
}

fn embedder_config(format: LogFormat, signals: Option<OtelSignals>) -> anyhow::Result<TelemetryConfig> {
    let writer = if format.is_json() {
        BoxMakeWriter::new(std::io::stdout)
    } else if signals.is_some() {
        BoxMakeWriter::new(std::io::stderr)
    } else {
        BoxMakeWriter::new(progress_log_writer as fn() -> ProgressLogWriter)
    };
    let otlp = signals
        .map(|signals| {
            let filter = if signals.0.contains(&OtelSignal::Traces) || signals.0.contains(&OtelSignal::Logs) {
                amaru_trace_filter()?
            } else {
                EnvFilter::new("off")
            };
            Ok::<_, anyhow::Error>(OtlpConfig { resource: resource(None), signals, filter })
        })
        .transpose()?;
    Ok(TelemetryConfig {
        otlp,
        output: Some(LogOutput { format, writer, filter: rust_log_filter(), span_events: FmtSpan::NONE }),
        existing_subscriber: ExistingSubscriber::Keep,
        ..TelemetryConfig::default()
    })
}

/// Build the shared OTLP resource, with an optional caller-resolved service instance ID.
///
/// `OTEL_SERVICE_NAME` overrides the default `amaru` name. SDK resource detection
/// supplies other attributes, including `OTEL_RESOURCE_ATTRIBUTES`.
pub fn resource(service_instance_id: Option<String>) -> Resource {
    let service_name = env::var("OTEL_SERVICE_NAME")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_SERVICE_NAME.to_string());
    let mut attributes = vec![KeyValue::new(SERVICE_NAME, service_name)];
    if let Some(instance_id) = service_instance_id {
        attributes.push(KeyValue::new(SERVICE_INSTANCE_ID, instance_id));
    }
    Resource::builder().with_attributes(attributes).build()
}

fn amaru_trace_filter() -> anyhow::Result<EnvFilter> {
    let filter = env::var("AMARU_TRACE").unwrap_or_else(|_| DEFAULT_AMARU_TRACE.to_string());
    EnvFilter::try_new(filter).context("parse AMARU_TRACE filter")
}

fn rust_log_filter() -> EnvFilter {
    if let Ok(filter) = env::var("AMARU_TRACE") {
        return EnvFilter::try_new(filter).unwrap_or_else(|_| EnvFilter::new(DEFAULT_AMARU_TRACE));
    }
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_AMARU_TRACE))
}

fn env_flag(name: &str) -> bool {
    env::var(name).ok().is_some_and(|v| {
        let v = v.trim().to_ascii_lowercase();
        matches!(v.as_str(), "1" | "true" | "yes" | "on")
    })
}

#[cfg(test)]
mod tests {
    use std::{
        io::{self, Write},
        sync::{Arc, Mutex},
    };

    use amaru_kernel::{HeaderHash, Slot};
    use amaru_observability::info;
    use tracing_subscriber::fmt::MakeWriter;

    use super::*;

    const SLOT: u64 = 42_000;
    const HASH_BYTE: u8 = 0xab;

    #[derive(Clone, Default)]
    struct CaptureWriter {
        buffer: Arc<Mutex<Vec<u8>>>,
    }

    impl CaptureWriter {
        fn contents(&self) -> String {
            String::from_utf8_lossy(&self.buffer.lock().unwrap_or_else(std::sync::PoisonError::into_inner)).into_owned()
        }
    }

    impl Write for CaptureWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.buffer.lock().unwrap_or_else(std::sync::PoisonError::into_inner).write(buf)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for CaptureWriter {
        type Writer = CaptureWriter;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    fn output_config(writer: &CaptureWriter, format: LogFormat) -> TelemetryConfig {
        TelemetryConfig {
            output: Some(LogOutput {
                format,
                writer: BoxMakeWriter::new(writer.clone()),
                filter: EnvFilter::new("info"),
                span_events: FmtSpan::NONE,
            }),
            ..TelemetryConfig::default()
        }
    }

    fn emit_tip_adopt() {
        let slot = Slot::new(SLOT);
        let header_hash = HeaderHash::new([HASH_BYTE; 32]);
        info!(
            consensus::tip::ADOPT,
            slot,
            header_hash,
            block_height = 100_u64,
            max_block_height = 100_u64,
            suppressed = 0_u32,
        );
    }

    fn header_hash_hex() -> String {
        HeaderHash::new([HASH_BYTE; 32]).to_string()
    }

    /// `consensus::tip::ADOPT` is a typical `run_until` event: `slot` and `header_hash`
    /// travel as CBOR `record_bytes` (they are newtypes, not tracing primitives).
    #[test]
    fn json_stack_decodes_cbor_schema_fields_to_primitives() {
        let writer = CaptureWriter::default();
        let (subscriber, _handle) = output_config(&writer, LogFormat::Json).build().unwrap();

        tracing::subscriber::with_default(subscriber, emit_tip_adopt);

        let output = writer.contents();
        let line =
            output.lines().find(|l| l.contains("header_hash")).unwrap_or_else(|| panic!("json line, got: {output}"));
        let json: serde_json::Value = serde_json::from_str(line).unwrap_or_else(|e| panic!("json ({e}): {line}"));
        let fields = &json["fields"];

        assert_eq!(fields["slot"], SLOT, "CBOR Slot must be a JSON number, not diagnostic text or bytes: {line}");
        assert!(fields["slot"].is_number(), "slot must not be a string or byte array: {line}");
        assert_eq!(
            fields["header_hash"],
            header_hash_hex(),
            "CBOR HeaderHash must be a JSON string of hex, not diagnostic quotes or bytes: {line}"
        );
        assert!(fields["header_hash"].is_string(), "header_hash must be a string: {line}");
        assert_eq!(fields["block_height"], 100);
        assert_eq!(fields["max_block_height"], 100);
        assert_eq!(fields["suppressed"], 0);
    }

    #[test]
    fn console_stack_decodes_cbor_schema_fields_to_primitives() {
        let writer = CaptureWriter::default();
        let (subscriber, _handle) = output_config(&writer, LogFormat::Plain).build().unwrap();

        tracing::subscriber::with_default(subscriber, emit_tip_adopt);

        let output = writer.contents();
        let hash = header_hash_hex();
        assert!(output.contains(&format!("slot={SLOT}")), "CBOR Slot must print as an unquoted number: {output}");
        assert!(!output.contains(&format!("slot=\"{SLOT}\"")), "slot must not be diagnostic/quoted text: {output}");
        assert!(
            output.contains(&format!("header_hash=\"{hash}\"")),
            "CBOR HeaderHash must print as a single-quoted hex string: {output}"
        );
        assert!(
            !output.contains(&format!(r#"header_hash="\"{hash}\"""#)),
            "header_hash must not be diagnostic-quoted then Debug-quoted: {output}"
        );
        assert!(!output.contains('\u{1b}'), "plain console must not emit ANSI: {output}");
    }

    #[test]
    fn console_stack_emits_ansi_when_enabled() {
        let writer = CaptureWriter::default();
        let (subscriber, _handle) = output_config(&writer, LogFormat::Ansi).build().unwrap();

        tracing::subscriber::with_default(subscriber, emit_tip_adopt);

        let output = writer.contents();
        assert!(output.contains('\u{1b}'), "ANSI console must emit SGR escapes: {output}");
        assert!(output.contains(&SLOT.to_string()), "CBOR Slot must still print as a number: {output}");
    }
}
