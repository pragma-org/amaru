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

//! Shared subscriber assembly and exporter lifetime for the CLI and embedders.

use amaru_metrics::{Meter, MetricsEvent};
use amaru_observability::{
    CborJsonSpanLayer, CborTraceArrayLayer, SpanDurationLayer, TelemetryCaptureLayer, console_layer, json_layer,
};
use anyhow::Context;
use opentelemetry_sdk::Resource;
use tokio::task::JoinHandle;
use tracing::Subscriber;
use tracing_subscriber::{
    EnvFilter, Layer,
    fmt::{format::FmtSpan, writer::BoxMakeWriter},
    layer::SubscriberExt,
    registry::LookupSpan,
    util::SubscriberInitExt,
};

use super::{LogFormat, OpenTelemetryProviders, OtelSignals, TraceOutputSpec, file, filter::OtelErrorFilter};
use crate::system_metrics::{BuildIdentity, track_system_metrics};

/// OTLP export and the filter applied independently to its trace and log layers.
pub struct OtlpConfig {
    pub resource: Resource,
    pub signals: OtelSignals,
    pub filter: EnvFilter,
}

/// A local console or JSON sink. The caller chooses its writer and span lifecycle events.
pub struct LogOutput {
    pub format: LogFormat,
    pub writer: BoxMakeWriter,
    pub filter: EnvFilter,
    pub span_events: FmtSpan,
}

impl LogOutput {
    fn layer<S>(self, report_export_errors: bool) -> Box<dyn Layer<S> + Send + Sync>
    where
        S: Subscriber + for<'lookup> LookupSpan<'lookup> + 'static,
    {
        let filter = OtelErrorFilter::wrap(self.filter, report_export_errors);
        if self.format.is_json() {
            json_layer(self.writer, self.span_events).with_filter(filter).boxed()
        } else {
            console_layer(self.writer, self.format.ansi(), self.span_events).with_filter(filter).boxed()
        }
    }
}

/// Optional in-process sinks, supplied by the TUI or an embedding application.
#[derive(Default)]
pub struct LocalTelemetry {
    pub metrics_observer: Option<Box<dyn Fn(&MetricsEvent) + Send + Sync>>,
    pub capture_layer: Option<TelemetryCaptureLayer>,
}

/// How installation handles a subscriber already installed by the host process.
#[derive(Debug, Clone, Copy, Default)]
pub enum ExistingSubscriber {
    /// Report the installation conflict to the caller.
    #[default]
    Reject,
    /// Keep the host subscriber while retaining the configured metric exporter.
    Keep,
}

/// Configuration for the shared subscriber. Defaults enable no output or OTLP export.
pub struct TelemetryConfig {
    pub otlp: Option<OtlpConfig>,
    pub output: Option<LogOutput>,
    pub local: LocalTelemetry,
    pub capture_filter: EnvFilter,
    pub files: Vec<TraceOutputSpec>,
    /// Replace repeated SDK export errors with grouped failure and recovery notices.
    pub report_export_errors: bool,
    pub existing_subscriber: ExistingSubscriber,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            otlp: None,
            output: None,
            local: LocalTelemetry::default(),
            capture_filter: EnvFilter::new("info"),
            files: Vec::new(),
            report_export_errors: false,
            existing_subscriber: ExistingSubscriber::Reject,
        }
    }
}

impl TelemetryConfig {
    /// Build a subscriber without installing it globally.
    ///
    /// Keep the returned handle alive for as long as the subscriber is in use. OTLP
    /// construction requires a current Tokio runtime; shutdown must finish before that runtime stops.
    pub fn build(self) -> anyhow::Result<(impl Subscriber + Send + Sync, TelemetryHandle)> {
        let providers = self
            .otlp
            .as_ref()
            .map(|config| OpenTelemetryProviders::try_new(config.resource.clone(), &config.signals))
            .transpose()?;
        let mut meter = providers.as_ref().map(OpenTelemetryProviders::meter).unwrap_or_default();
        if let Some(observer) = self.local.metrics_observer {
            meter.set_local_observer(observer);
        }
        let handle = TelemetryHandle { meter, teardown: TelemetryGuard { providers, system_metrics: None } };
        let providers = handle.teardown.providers.as_ref();
        let filter = |inner| OtelErrorFilter::wrap(inner, self.report_export_errors);
        let otlp_filter = self.otlp.map(|config| config.filter).unwrap_or_else(|| EnvFilter::new("off"));
        let trace = providers
            .and_then(OpenTelemetryProviders::trace_layer)
            .map(|layer| layer.with_filter(filter(otlp_filter.clone())));
        let logs =
            providers.and_then(OpenTelemetryProviders::log_layer).map(|layer| layer.with_filter(filter(otlp_filter)));
        let json = self.output.as_ref().is_some_and(|output| output.format.is_json());
        let subscriber = tracing_subscriber::registry()
            .with(trace)
            .with(providers.map(|_| CborTraceArrayLayer::new()))
            .with(logs)
            .with(self.local.capture_layer.map(|layer| layer.with_filter(filter(self.capture_filter))))
            .with(json.then(CborJsonSpanLayer::new));
        let subscriber = file::attach(subscriber, &self.files)?
            .with(SpanDurationLayer::new())
            .with(self.output.map(|output| output.layer(self.report_export_errors)));
        Ok((subscriber, handle))
    }

    /// Install the shared subscriber and return ownership of its exporters.
    pub fn install(self) -> anyhow::Result<TelemetryHandle> {
        let existing = self.existing_subscriber;
        let (subscriber, handle) = self.build()?;
        match subscriber.try_init() {
            Ok(()) => Ok(handle),
            Err(_) if matches!(existing, ExistingSubscriber::Keep) && tracing::dispatcher::has_been_set() => Ok(handle),
            Err(error) => Err(error).context("install telemetry subscriber"),
        }
    }
}

/// A meter that can be passed to the node, and ownership of the associated exporter lifetime.
#[derive(Default)]
pub struct TelemetryHandle {
    pub meter: Meter,
    pub teardown: TelemetryGuard,
}

/// Owns exporter shutdown and an optional process-metrics poller.
#[derive(Default)]
pub struct TelemetryGuard {
    providers: Option<OpenTelemetryProviders>,
    system_metrics: Option<JoinHandle<()>>,
}

impl TelemetryGuard {
    pub(super) fn collect_system_metrics(&mut self, meter: std::sync::Arc<Meter>) -> anyhow::Result<()> {
        self.system_metrics = track_system_metrics(meter, BuildIdentity::default()).context("start system metrics")?;
        Ok(())
    }

    /// Stop collection and flush exporters once, while their Tokio runtime is still alive.
    pub fn shutdown(mut self) -> anyhow::Result<()> {
        self.stop()
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        if let Some(handle) = self.system_metrics.take() {
            handle.abort();
        }
        if let Some(providers) = self.providers.take() {
            providers.shutdown()?;
        }
        Ok(())
    }
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            eprintln!("amaru-node telemetry teardown failed: {error}");
        }
    }
}
