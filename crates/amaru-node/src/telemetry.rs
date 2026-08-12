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
//! Used by thin programs such as the `run_until` example so OTLP metrics/traces
//! can be enabled without the product CLI. When OTLP is on, also starts
//! process/build gauges via [`crate::system_metrics`]. The product binary still
//! uses its own richer stack in `amaru::observability` (TUI capture layer, local
//! metrics observer, ANSI colour, throttled OTEL filters, service instance id,
//! dual `AMARU_LOG` / `AMARU_TRACE` filters, delayed filter warnings).
//!
//! [`Telemetry::install`] and [`Telemetry::shutdown`] are `async` so they must
//! be driven on a Tokio runtime (`rt.block_on(...)` or `.await` inside an async
//! context). OTLP batch exporters attach to that runtime; do not use
//! `Handle::enter()` / `Runtime::enter()` around these calls.

use std::{env, sync::Arc};

use amaru_metrics::{METRICS_METER_NAME, Meter};
use anyhow::{Context, anyhow};
use opentelemetry::{KeyValue, metrics::MeterProvider as _, trace::TracerProvider as _};
use opentelemetry_sdk::{
    Resource,
    logs::SdkLoggerProvider,
    metrics::{SdkMeterProvider, Temporality},
    trace::SdkTracerProvider,
};
use opentelemetry_semantic_conventions::resource::SERVICE_NAME;
use tokio::task::JoinHandle;
use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt, util::SubscriberInitExt};

use crate::system_metrics::{BuildIdentity, track_system_metrics};

const DEFAULT_SERVICE_NAME: &str = "amaru";
const DEFAULT_AMARU_TRACE: &str = "info";

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
    /// - `AMARU_WITH_JSON_TRACES` — when truthy, emit JSON log lines on stdout
    ///   (used by trace-contract checks).
    /// - `AMARU_TRACE` — filter for Amaru event targets (default `info`).
    /// - `RUST_LOG` — optional extra filter for non-schema logging.
    /// - `OTEL_SERVICE_NAME` — resource service name (default `amaru`).
    ///
    /// When OTLP is disabled, installs a compact stderr fmt layer only and a
    /// default empty [`Meter`].
    ///
    /// Must be polled on a Tokio runtime (OTLP batch exporters spawn on it).
    pub async fn install() -> anyhow::Result<Self> {
        let with_otlp = env_flag("AMARU_WITH_OPEN_TELEMETRY");
        let with_json = env_flag("AMARU_WITH_JSON_TRACES");

        // Yield once so this future is clearly runtime-bound; batch exporters
        // also require a current runtime handle when constructed below.
        tokio::task::yield_now().await;

        if with_otlp { Self::install_otlp(with_json) } else { Self::install_local(with_json) }
    }

    fn install_local(with_json: bool) -> anyhow::Result<Self> {
        init_fmt_subscriber(with_json)?;
        Ok(Self { meter: Arc::new(Meter::default()), system_metrics: None, teardown: None })
    }

    fn install_otlp(with_json: bool) -> anyhow::Result<Self> {
        let resource = build_resource();
        let service_name = resource
            .get(&opentelemetry::Key::from_static_str(SERVICE_NAME))
            .map(|v| v.as_str().to_string())
            .unwrap_or_else(|| DEFAULT_SERVICE_NAME.to_string());

        let traces_provider = SdkTracerProvider::builder()
            .with_resource(resource.clone())
            .with_batch_exporter(
                opentelemetry_otlp::SpanExporter::builder().with_tonic().build().context("build OTLP span exporter")?,
            )
            .build();

        let logs_provider = SdkLoggerProvider::builder()
            .with_resource(resource.clone())
            .with_batch_exporter(
                opentelemetry_otlp::LogExporter::builder().with_tonic().build().context("build OTLP log exporter")?,
            )
            .build();

        let metric_exporter = opentelemetry_otlp::MetricExporter::builder()
            .with_tonic()
            .with_temporality(Temporality::default())
            .build()
            .context("build OTLP metric exporter")?;

        let meter_provider = SdkMeterProvider::builder()
            .with_resource(resource)
            .with_reader(opentelemetry_sdk::metrics::PeriodicReader::builder(metric_exporter).build())
            .build();

        let otel_layer = tracing_opentelemetry::layer()
            .with_tracer(traces_provider.tracer(service_name))
            .with_level(true)
            .with_target(true)
            .with_filter(amaru_trace_filter()?);

        let log_bridge = opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(&logs_provider)
            .with_filter(amaru_trace_filter()?);

        let fmt_filter = rust_log_filter();
        if with_json {
            let fmt = tracing_subscriber::fmt::layer()
                .json()
                .with_span_list(false)
                .with_writer(std::io::stdout)
                .with_filter(fmt_filter);
            tracing_subscriber::registry()
                .with(otel_layer)
                .with(log_bridge)
                .with(fmt)
                .try_init()
                .context("init OTLP+JSON tracing subscriber")?;
        } else {
            let fmt = tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(false)
                .compact()
                .with_filter(fmt_filter);
            tracing_subscriber::registry()
                .with(otel_layer)
                .with(log_bridge)
                .with(fmt)
                .try_init()
                .context("init OTLP+fmt tracing subscriber")?;
        }

        let meter = Arc::new(Meter::from(meter_provider.meter(METRICS_METER_NAME)));
        // Same process/build gauges as the product binary so e2e compare-metrics
        // sees the full supported set when OTLP is enabled.
        let system_metrics = track_system_metrics(Arc::clone(&meter), BuildIdentity::default())
            .map_err(|e| anyhow!("start system metrics: {e}"))?;

        let teardown = Box::new(move || {
            traces_provider.shutdown().map_err(|e| anyhow!("trace provider shutdown: {e}"))?;
            meter_provider.shutdown().map_err(|e| anyhow!("meter provider shutdown: {e}"))?;
            logs_provider.shutdown().map_err(|e| anyhow!("log provider shutdown: {e}"))?;
            Ok(())
        });

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

fn init_fmt_subscriber(with_json: bool) -> anyhow::Result<()> {
    let filter = rust_log_filter();
    if with_json {
        tracing_subscriber::fmt()
            .json()
            .with_span_list(false)
            .with_writer(std::io::stdout)
            .with_env_filter(filter)
            .try_init()
            .map_err(|e| anyhow!("init JSON tracing subscriber: {e}"))?;
    } else {
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_ansi(false)
            .compact()
            .with_env_filter(filter)
            .try_init()
            .map_err(|e| anyhow!("init fmt tracing subscriber: {e}"))?;
    }
    Ok(())
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
