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

//! Embedder-facing observability install helpers.
//!
//! Used by thin programs such as the `run_until` example so OTLP metrics, traces,
//! and logs can be enabled without the product CLI. When OTLP metrics are on,
//! process/build gauges are collected by default; [`TelemetryOptions`] can disable
//! their sysinfo poller.
//!
//! Console, JSON, and OTEL log/trace layers are the same CBOR-aware formatters
//! as the product binary (`CborConsoleEventFormat`, `CborJsonEventFormat`,
//! `CborTraceArrayLayer`, `CborOtelLogBridge`) so schema `record_bytes` fields
//! decode to numbers and strings where the sink can hold them. Local console
//! colour is selected with [`LogFormat`] (`Plain` / `Ansi` / `Json` — JSON is
//! exclusive with colour). The product stack in `amaru::observability` still
//! adds TUI capture, a local metrics observer, throttled OTEL filters, service
//! instance id, dual `AMARU_LOG` / `AMARU_TRACE` filters, and delayed filter
//! warnings.
//!
//! [`Telemetry::install`] and [`Telemetry::shutdown`] are `async` so they must
//! be driven on a Tokio runtime (`rt.block_on(...)` or `.await` inside an async
//! context). OTLP batch exporters attach to that runtime; do not use
//! `Handle::enter()` / `Runtime::enter()` around these calls.

use std::{env, sync::Arc};

use amaru_metrics::{METRICS_METER_NAME, Meter};
use anyhow::{Context, anyhow};
use opentelemetry::{KeyValue, metrics::MeterProvider as _, trace::TracerProvider as _};
use opentelemetry_sdk::Resource;
use opentelemetry_semantic_conventions::resource::SERVICE_NAME;
use tokio::task::JoinHandle;
use tracing::Subscriber;
use tracing_subscriber::{
    EnvFilter, Layer, fmt::MakeWriter, layer::SubscriberExt, registry::LookupSpan, util::SubscriberInitExt,
};

use crate::{
    observability::{
        CborConsoleEventFormat, CborJsonEventFormat, CborJsonFields, CborJsonSpanLayer, CborOtelLogBridge,
        CborTraceArrayLayer, console_field_formatter,
    },
    system_metrics::{BuildIdentity, track_system_metrics},
};

mod open_telemetry;

pub use open_telemetry::{
    BuildOpenTelemetryProvidersError, OpenTelemetryProviders, OtelSignal, OtelSignals,
    ShutdownOpenTelemetryProvidersError,
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
    system_metrics: Option<JoinHandle<()>>,
    teardown: Option<Box<dyn FnOnce() -> anyhow::Result<()> + Send>>,
}

impl Telemetry {
    /// Install process-wide tracing and optional OTLP export.
    ///
    /// Environment:
    /// - `AMARU_WITH_OPEN_TELEMETRY` — when truthy (`1`/`true`/`yes`/`on`), export
    ///   metrics, traces, and logs via OTLP (see `OTEL_EXPORTER_OTLP_ENDPOINT`).
    /// - `AMARU_WITH_JSON_TRACES` — when truthy, forces [`LogFormat::Json`]
    ///   regardless of `format` (used by trace-contract checks).
    /// - `AMARU_TRACE` — filter for Amaru event targets (default `info`).
    /// - `RUST_LOG` — optional extra filter for non-schema logging.
    /// - `OTEL_SERVICE_NAME` — resource service name (default `amaru`).
    ///
    /// When OTLP is disabled, installs a CBOR-aware stderr (or JSON stdout)
    /// layer only and a default empty [`Meter`].
    ///
    /// Must be polled on a Tokio runtime (OTLP batch exporters spawn on it).
    pub async fn install(format: LogFormat) -> anyhow::Result<Self> {
        Self::install_with_options(format, TelemetryOptions::default()).await
    }

    /// Install telemetry with explicit control over optional collectors.
    ///
    /// Set [`TelemetryOptions::collect_system_metrics`] to `false` on platforms
    /// where sysinfo collection is unavailable, including iOS.
    pub async fn install_with_options(format: LogFormat, options: TelemetryOptions) -> anyhow::Result<Self> {
        let with_otlp = env_flag("AMARU_WITH_OPEN_TELEMETRY");
        let format = if env_flag("AMARU_WITH_JSON_TRACES") { LogFormat::Json } else { format };

        // Yield once so this future is clearly runtime-bound; batch exporters
        // also require a current runtime handle when constructed below.
        tokio::task::yield_now().await;

        if with_otlp { Self::install_otlp(format, options) } else { Self::install_local(format) }
    }

    fn accept_already_set(result: Result<(), impl std::fmt::Display>, what: &str) -> anyhow::Result<()> {
        match result {
            Ok(()) => Ok(()),
            Err(_) if tracing::dispatcher::has_been_set() => Ok(()),
            Err(e) => Err(anyhow!("{what}: {e}")),
        }
    }

    /// Install a local fmt / JSON subscriber without OTLP export.
    ///
    /// [`LogFormat::Ansi`] colours stderr; [`LogFormat::Json`] is exclusive with
    /// colour and writes NDJSON to stdout.
    pub fn install_local(format: LogFormat) -> anyhow::Result<Self> {
        init_fmt_subscriber(format)?;
        Ok(Self { meter: Arc::new(Meter::default()), system_metrics: None, teardown: None })
    }

    fn install_otlp(format: LogFormat, options: TelemetryOptions) -> anyhow::Result<Self> {
        let signals = &options.open_telemetry_signals;
        let resource = build_resource();
        let providers = OpenTelemetryProviders::try_new(resource, signals)?;
        let meter = Arc::new(
            providers
                .meter_provider()
                .map(|provider| Meter::from(provider.meter(METRICS_METER_NAME)))
                .unwrap_or_default(),
        );

        let otel_layer = providers
            .tracer_provider()
            .map(|provider| {
                amaru_trace_filter().map(|filter| {
                    tracing_opentelemetry::layer()
                        .with_tracer(provider.tracer(providers.service_name().to_string()))
                        .with_level(true)
                        .with_target(true)
                        .with_filter(filter)
                })
            })
            .transpose()?;

        // Project-owned bridge so CBOR field payloads become nested AnyValue maps/lists
        // (or scalars) instead of opaque bytes / diagnostic text.
        let log_bridge = providers
            .logger_provider()
            .map(|provider| amaru_trace_filter().map(|filter| CborOtelLogBridge::new(provider).with_filter(filter)))
            .transpose()?;

        let fmt_filter = rust_log_filter();
        if format.is_json() {
            Self::accept_already_set(
                tracing_subscriber::registry()
                    .with(otel_layer)
                    .with(CborTraceArrayLayer::new())
                    .with(log_bridge)
                    .with(CborJsonSpanLayer::new())
                    .with(json_fmt_layer(std::io::stdout).with_filter(fmt_filter))
                    .try_init(),
                "init OTLP+JSON tracing subscriber",
            )?;
        } else {
            Self::accept_already_set(
                tracing_subscriber::registry()
                    .with(otel_layer)
                    .with(CborTraceArrayLayer::new())
                    .with(log_bridge)
                    .with(console_fmt_layer(std::io::stderr, format.ansi()).with_filter(fmt_filter))
                    .try_init(),
                "init OTLP+fmt tracing subscriber",
            )?;
        }

        // Same process/build gauges as the product binary so e2e compare-metrics
        // sees the full supported set when OTLP is enabled.
        let system_metrics = if signals.0.contains(&OtelSignal::Metrics) && options.collect_system_metrics {
            track_system_metrics(Arc::clone(&meter), BuildIdentity::default()).context("start system metrics")?
        } else {
            None
        };

        let teardown = Box::new(move || Ok(providers.shutdown()?));

        Ok(Self { meter, system_metrics, teardown: Some(teardown) })
    }

    /// Flush exporters. Safe to call once at process exit.
    ///
    /// Must be polled on a Tokio runtime so provider shutdown can complete
    /// exporter work that was scheduled on that runtime.
    pub async fn shutdown(mut self) -> anyhow::Result<()> {
        tokio::task::yield_now().await;
        if let Some(handle) = self.system_metrics.take() {
            handle.abort();
        }
        if let Some(teardown) = self.teardown.take() {
            teardown()?;
        }
        Ok(())
    }
}

impl Drop for Telemetry {
    fn drop(&mut self) {
        if let Some(handle) = self.system_metrics.take() {
            handle.abort();
        }
        if let Some(teardown) = self.teardown.take()
            && let Err(err) = teardown()
        {
            eprintln!("amaru-node telemetry teardown failed: {err}");
        }
    }
}

fn init_fmt_subscriber(format: LogFormat) -> anyhow::Result<()> {
    let filter = rust_log_filter();
    if format.is_json() {
        Telemetry::accept_already_set(
            tracing_subscriber::registry()
                .with(CborJsonSpanLayer::new())
                .with(json_fmt_layer(std::io::stdout).with_filter(filter))
                .try_init(),
            "init JSON tracing subscriber",
        )?;
    } else {
        Telemetry::accept_already_set(
            tracing_subscriber::registry()
                .with(console_fmt_layer(std::io::stderr, format.ansi()).with_filter(filter))
                .try_init(),
            "init fmt tracing subscriber",
        )?;
    }
    Ok(())
}

/// Console sink used by [`Telemetry::install`]: decode CBOR `record_bytes` to
/// native visit types and hide schema tags (EDR-033).
fn console_fmt_layer<S, W>(writer: W, ansi: bool) -> impl Layer<S>
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    W: for<'writer> MakeWriter<'writer> + Send + Sync + 'static,
{
    tracing_subscriber::fmt::layer()
        .with_writer(writer)
        .with_ansi(ansi)
        .fmt_fields(console_field_formatter())
        .event_format(CborConsoleEventFormat::new().with_ansi(ansi))
}

/// JSON sink used by [`Telemetry::install`]: CBOR scalars become JSON numbers /
/// strings / bools; maps and arrays become nested JSON.
fn json_fmt_layer<S, W>(writer: W) -> impl Layer<S>
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    W: for<'writer> MakeWriter<'writer> + Send + Sync + 'static,
{
    tracing_subscriber::fmt::layer()
        .with_writer(writer)
        .event_format(CborJsonEventFormat::new())
        .fmt_fields(CborJsonFields::new())
}

fn build_resource() -> Resource {
    let service_name = env::var("OTEL_SERVICE_NAME")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_SERVICE_NAME.to_string());

    Resource::builder().with_attributes([KeyValue::new(SERVICE_NAME, service_name)]).build()
}

fn amaru_trace_filter() -> anyhow::Result<EnvFilter> {
    let filter = env::var("AMARU_TRACE").unwrap_or_else(|_| DEFAULT_AMARU_TRACE.to_string());
    EnvFilter::try_new(filter).context("parse AMARU_TRACE filter")
}

fn rust_log_filter() -> EnvFilter {
    // Prefer AMARU_TRACE for schema events; fall back to RUST_LOG then a quiet default.
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
    use tracing_subscriber::layer::SubscriberExt;

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
        let subscriber =
            tracing_subscriber::registry().with(CborJsonSpanLayer::new()).with(json_fmt_layer(writer.clone()));

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
        let subscriber = tracing_subscriber::registry().with(console_fmt_layer(writer.clone(), false));

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
        let subscriber = tracing_subscriber::registry().with(console_fmt_layer(writer.clone(), true));

        tracing::subscriber::with_default(subscriber, emit_tip_adopt);

        let output = writer.contents();
        assert!(output.contains('\u{1b}'), "ANSI console must emit SGR escapes: {output}");
        assert!(output.contains(&SLOT.to_string()), "CBOR Slot must still print as a number: {output}");
    }
}
