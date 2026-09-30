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

use std::{str::FromStr, time::Duration};

use amaru_observability::{offer_span_elapsed, prepare_exported_attributes};
use opentelemetry::{Context, Key};
use opentelemetry_otlp::ExporterBuildError;
use opentelemetry_sdk::{
    Resource,
    error::{OTelSdkError, OTelSdkResult},
    logs::SdkLoggerProvider,
    metrics::{SdkMeterProvider, Temporality},
    trace::{BatchSpanProcessor, SdkTracerProvider, Span, SpanData, SpanExporter, SpanProcessor},
};
use opentelemetry_semantic_conventions::resource::SERVICE_NAME;
use thiserror::Error;

/// Replaces a stock CBOR debug dump with the decoded attribute and keeps one value per key.
#[derive(Debug)]
pub struct CborSpanExporter<E> {
    inner: E,
}

impl<E> CborSpanExporter<E> {
    pub fn new(inner: E) -> Self {
        Self { inner }
    }
}

impl<E> SpanExporter for CborSpanExporter<E>
where
    E: SpanExporter,
{
    fn export(&self, mut batch: Vec<SpanData>) -> impl std::future::Future<Output = OTelSdkResult> + Send {
        for span in &mut batch {
            prepare_exported_attributes(&mut span.attributes, &mut span.events.events);
        }
        self.inner.export(batch)
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.inner.shutdown_with_timeout(timeout)
    }

    fn force_flush(&self) -> OTelSdkResult {
        self.inner.force_flush()
    }
}

/// Notes a `perf` span's exported elapsed time before the inner processor queues or exports it.
///
/// `on_end` runs while the tracing span is closing, including when the inner processor is a
/// batch exporter that ships the span later.
#[derive(Debug)]
pub(crate) struct NotingSpanProcessor<P> {
    inner: P,
}

impl<P> NotingSpanProcessor<P> {
    pub(crate) fn new(inner: P) -> Self {
        Self { inner }
    }
}

impl<P> SpanProcessor for NotingSpanProcessor<P>
where
    P: SpanProcessor,
{
    fn on_start(&self, span: &mut Span, cx: &Context) {
        self.inner.on_start(span, cx);
    }

    fn on_end(&self, span: SpanData) {
        if span.name.starts_with("perf.") {
            let micros = span.end_time.duration_since(span.start_time).unwrap_or_default().as_micros() as u64;
            offer_span_elapsed(&span.name, micros);
        }
        self.inner.on_end(span);
    }

    fn force_flush(&self) -> OTelSdkResult {
        self.inner.force_flush()
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.inner.shutdown_with_timeout(timeout)
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.inner.set_resource(resource);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtelSignal {
    Metrics,
    Traces,
    Logs,
}

impl OtelSignal {
    const ALL: [Self; 3] = [Self::Metrics, Self::Traces, Self::Logs];
}

impl FromStr for OtelSignal {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "metrics" => Ok(Self::Metrics),
            "traces" => Ok(Self::Traces),
            "logs" => Ok(Self::Logs),
            _ => Err("expected one of: metrics, traces, logs"),
        }
    }
}

/// The OpenTelemetry signals enabled for OTLP export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtelSignals(pub Vec<OtelSignal>);

impl Default for OtelSignals {
    fn default() -> Self {
        Self(OtelSignal::ALL.to_vec())
    }
}

impl From<Vec<OtelSignal>> for OtelSignals {
    fn from(signals: Vec<OtelSignal>) -> Self {
        Self(signals)
    }
}

/// The OTLP providers selected for a process.
///
/// This owns provider construction and shutdown so the product binary and
/// embedders share the same exporter lifecycle while remaining free to install
/// different subscriber layers.
pub struct OpenTelemetryProviders {
    service_name: String,
    traces: Option<SdkTracerProvider>,
    metrics: Option<SdkMeterProvider>,
    logs: Option<SdkLoggerProvider>,
}

impl OpenTelemetryProviders {
    /// Construct only the providers selected by `signals`.
    pub fn try_new(resource: Resource, signals: &OtelSignals) -> Result<Self, BuildOpenTelemetryProvidersError> {
        let service_name = resource
            .get(&Key::from_static_str(SERVICE_NAME))
            .ok_or(BuildOpenTelemetryProvidersError::MissingServiceName)?
            .as_str()
            .to_string();

        let traces = signals
            .0
            .contains(&OtelSignal::Traces)
            .then(|| {
                let exporter = opentelemetry_otlp::SpanExporter::builder()
                    .with_tonic()
                    .build()
                    .map_err(BuildOpenTelemetryProvidersError::Traces)?;
                let batch = BatchSpanProcessor::builder(CborSpanExporter::new(exporter)).build();
                Ok(SdkTracerProvider::builder()
                    .with_resource(resource.clone())
                    .with_span_processor(NotingSpanProcessor::new(batch))
                    .build())
            })
            .transpose()?;

        let logs = signals
            .0
            .contains(&OtelSignal::Logs)
            .then(|| {
                let exporter = opentelemetry_otlp::LogExporter::builder()
                    .with_tonic()
                    .build()
                    .map_err(BuildOpenTelemetryProvidersError::Logs)?;
                Ok(SdkLoggerProvider::builder().with_resource(resource.clone()).with_batch_exporter(exporter).build())
            })
            .transpose()?;

        let metrics = signals
            .0
            .contains(&OtelSignal::Metrics)
            .then(|| {
                let exporter = opentelemetry_otlp::MetricExporter::builder()
                    .with_tonic()
                    .with_temporality(Temporality::default())
                    .build()
                    .map_err(BuildOpenTelemetryProvidersError::Metrics)?;
                Ok(SdkMeterProvider::builder()
                    .with_resource(resource)
                    .with_reader(opentelemetry_sdk::metrics::PeriodicReader::builder(exporter).build())
                    .build())
            })
            .transpose()?;

        Ok(Self { service_name, traces, metrics, logs })
    }

    /// Return the `service.name` captured from the provider resource.
    pub fn service_name(&self) -> &str {
        &self.service_name
    }

    /// Return the trace provider when trace export is enabled.
    pub fn tracer_provider(&self) -> Option<&SdkTracerProvider> {
        self.traces.as_ref()
    }

    /// Return the metric provider when metric export is enabled.
    pub fn meter_provider(&self) -> Option<&SdkMeterProvider> {
        self.metrics.as_ref()
    }

    /// Return the log provider when log export is enabled.
    pub fn logger_provider(&self) -> Option<&SdkLoggerProvider> {
        self.logs.as_ref()
    }

    /// Flush and shut down every constructed provider.
    pub fn shutdown(self) -> Result<(), ShutdownOpenTelemetryProvidersError> {
        use ShutdownOpenTelemetryProvidersError as ShutdownError;

        let traces = shutdown_provider(self.traces, SdkTracerProvider::shutdown, ShutdownError::Traces);
        let metrics = shutdown_provider(self.metrics, SdkMeterProvider::shutdown, ShutdownError::Metrics);
        let logs = shutdown_provider(self.logs, SdkLoggerProvider::shutdown, ShutdownError::Logs);

        traces.and(metrics).and(logs)
    }
}

/// Failure while constructing one of the selected OTLP providers.
#[derive(Debug, Error)]
pub enum BuildOpenTelemetryProvidersError {
    #[error("OpenTelemetry resource is missing `service.name`")]
    MissingServiceName,
    #[error("failed to build OTLP span exporter")]
    Traces(#[source] ExporterBuildError),
    #[error("failed to build OTLP metric exporter")]
    Metrics(#[source] ExporterBuildError),
    #[error("failed to build OTLP log exporter")]
    Logs(#[source] ExporterBuildError),
}

/// Failure while shutting down one of the selected OTLP providers.
#[derive(Debug, Error)]
pub enum ShutdownOpenTelemetryProvidersError {
    #[error("failed to shut down OpenTelemetry tracer provider")]
    Traces(#[source] OTelSdkError),
    #[error("failed to shut down OpenTelemetry meter provider")]
    Metrics(#[source] OTelSdkError),
    #[error("failed to shut down OpenTelemetry log provider")]
    Logs(#[source] OTelSdkError),
}

fn shutdown_provider<P>(
    provider: Option<P>,
    shutdown: fn(&P) -> Result<(), OTelSdkError>,
    map_error: fn(OTelSdkError) -> ShutdownOpenTelemetryProvidersError,
) -> Result<(), ShutdownOpenTelemetryProvidersError> {
    match provider {
        Some(provider) => shutdown(&provider).map_err(map_error),
        None => Ok(()),
    }
}
