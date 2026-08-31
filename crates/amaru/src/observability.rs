// Copyright 2025 PRAGMA
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

use std::{
    env::{VarError, var},
    error::Error,
    io::{self, IsTerminal},
    str::FromStr,
    sync::{OnceLock, mpsc},
    time::{Duration, Instant},
};

use amaru_metrics::{METRICS_METER_NAME, Meter};
use amaru_observability::{
    CborConsoleEventFormat, CborJsonEventFormat, CborJsonFields, CborJsonSpanLayer, CborOtelLogBridge,
    CborTraceArrayLayer, OpenTelemetrySignals, TelemetryCaptureLayer, console_field_formatter, info,
    tracing::{Metadata, Subscriber, level_filters::LevelFilter, span, subscriber::Interest},
    tracing_opentelemetry,
    tracing_subscriber::{
        self, EnvFilter, Registry,
        filter::Filtered,
        fmt::{Layer, format::FmtSpan},
        layer::{Context, Filter, Layered, SubscriberExt},
        prelude::*,
        util::SubscriberInitExt,
    },
    warn,
};
use opentelemetry::{Key, KeyValue, metrics::MeterProvider, trace::TracerProvider};
use opentelemetry_sdk::{
    Resource,
    logs::SdkLoggerProvider,
    metrics::{SdkMeterProvider, Temporality},
    trace::SdkTracerProvider,
};
use opentelemetry_semantic_conventions::resource::{SERVICE_INSTANCE_ID, SERVICE_NAME};

const AMARU_LOG_VAR: &str = "AMARU_LOG";

const DEFAULT_AMARU_LOG_FILTER: &str = "error,amaru=info,amaru_pure_stage=warn";

const AMARU_TRACE_VAR: &str = "AMARU_TRACE";

const DEFAULT_AMARU_TRACE_FILTER: &str = "error,amaru=debug,amaru_pure_stage=warn";

const OTEL_EXPORT_TRANSITION_DELAY: Duration = Duration::from_secs(5);

static OTEL_EXPORT_REPORTER: OnceLock<OtelExportReporter> = OnceLock::new();

// -----------------------------------------------------------------------------
// TracingSubscriber
// -----------------------------------------------------------------------------

/// Registry → OTEL trace layer.
type AfterTrace<S> = Layered<Option<OpenTelemetryFilter<S>>, S>;

/// After trace: upgrade CBOR homogeneous arrays on spans to `Value::Array`.
type AfterTraceArrays<S> = Layered<CborTraceArrayLayer, AfterTrace<S>>;

type OpenTelemetryLayer<S> = Layered<LogBridgeFilter<S>, AfterTraceArrays<S>>;

type LogBridgeFilter<S> = Option<
    Filtered<
        CborOtelLogBridge<SdkLoggerProvider, opentelemetry_sdk::logs::SdkLogger>,
        OtelErrorFilter,
        AfterTraceArrays<S>,
    >,
>;

type OpenTelemetryFilter<S> =
    Filtered<tracing_opentelemetry::OpenTelemetryLayer<S, opentelemetry_sdk::trace::Tracer>, OtelErrorFilter, S>;

/// Registry (or OTEL stack) with structured JSON span-field bag.
type WithJsonSpanFields<S> = Layered<CborJsonSpanLayer, S>;

type JsonLayer<S> = Layered<JsonFilter<S>, WithJsonSpanFields<S>>;

type JsonFilter<S> =
    Filtered<Layer<WithJsonSpanFields<S>, CborJsonFields, CborJsonEventFormat>, OtelErrorFilter, WithJsonSpanFields<S>>;

type LocalTelemetryFilter<S> = Filtered<TelemetryCaptureLayer, OtelErrorFilter, S>;

type LocalTelemetryLayer<S> = Layered<LocalTelemetryFilter<S>, S>;

type DelayedWarning = Option<Box<dyn FnOnce()>>;

// JSON event formatting (single-pass CBOR-aware NDJSON) lives in
// `amaru_observability::json_format`. Console field formatting (CBOR decode +
// tag hiding) lives in `amaru_observability::layers`. Nested-span encoding
// across stacks is specified in EDR-033.

#[derive(Default)]
pub enum TracingSubscriber<S> {
    #[default]
    Empty,
    Registry(Registry),
    WithOpenTelemetry(OpenTelemetryLayer<S>),
    WithLocalTelemetry(LocalTelemetryLayer<S>),
    WithLocalTelemetryAndOpenTelemetry(LocalTelemetryLayer<OpenTelemetryLayer<S>>),
    WithJson(JsonLayer<S>),
    WithJsonAndOpenTelemetry(JsonLayer<OpenTelemetryLayer<S>>),
}

impl TracingSubscriber<Registry> {
    pub fn new() -> Self {
        Self::Registry(tracing_subscriber::registry())
    }

    #[expect(clippy::panic)]
    #[expect(clippy::wildcard_enum_match_arm)]
    pub fn with_open_telemetry(
        &mut self,
        layer: Option<OpenTelemetryFilter<Registry>>,
        log_bridge: LogBridgeFilter<Registry>,
    ) {
        match std::mem::take(self) {
            Self::Registry(registry) => {
                *self = TracingSubscriber::WithOpenTelemetry(
                    registry.with(layer).with(CborTraceArrayLayer::new()).with(log_bridge),
                );
            }
            _ => panic!("'with_open_telemetry' called after 'with_json' or another terminal layer"),
        }
    }

    /// Install a local in-process telemetry capture layer (TUI or embedder subscription).
    ///
    /// Accepts the shared [`TelemetryCaptureLayer`] from `amaru-observability` so product setup
    /// does not depend on TUI types.
    #[expect(clippy::panic)]
    #[expect(clippy::wildcard_enum_match_arm)]
    pub fn with_local_telemetry(&mut self, layer: TelemetryCaptureLayer) -> DelayedWarning {
        let (default_filter, warning) = new_trace_filter();

        match std::mem::take(self) {
            Self::Registry(registry) => {
                *self = TracingSubscriber::WithLocalTelemetry(registry.with(layer.with_filter(default_filter)));
            }
            Self::WithOpenTelemetry(layered) => {
                *self = TracingSubscriber::WithLocalTelemetryAndOpenTelemetry(
                    layered.with(layer.with_filter(default_filter)),
                );
            }
            _ => panic!("'with_local_telemetry' called after as third layer"),
        }

        warning
    }

    #[expect(clippy::panic)]
    #[expect(clippy::wildcard_enum_match_arm)]
    pub fn with_json<F, G>(&mut self, registry_layer: F, otel_layer: G) -> DelayedWarning
    where
        F: FnOnce() -> (JsonFilter<Registry>, DelayedWarning),
        G: FnOnce() -> (JsonFilter<OpenTelemetryLayer<Registry>>, DelayedWarning),
    {
        match std::mem::take(self) {
            Self::Registry(registry) => {
                let (layer, warning) = registry_layer();
                *self = TracingSubscriber::WithJson(registry.with(CborJsonSpanLayer::new()).with(layer));
                warning
            }
            Self::WithOpenTelemetry(layered) => {
                let (layer, warning) = otel_layer();
                *self = TracingSubscriber::WithJsonAndOpenTelemetry(layered.with(CborJsonSpanLayer::new()).with(layer));
                warning
            }
            _ => panic!("'with_json' called after as third layer"),
        }
    }

    pub fn init(self, color: bool) -> DelayedWarning {
        match self {
            TracingSubscriber::Empty => unreachable!(),
            TracingSubscriber::Registry(registry) => {
                let (default_filter, warning) = new_log_filter();
                registry
                    .with(
                        tracing_subscriber::fmt::layer()
                            .with_writer(io::stderr as fn() -> io::Stderr)
                            .with_ansi(color)
                            .fmt_fields(console_field_formatter())
                            .with_span_events(FmtSpan::CLOSE)
                            .event_format(CborConsoleEventFormat::new().with_ansi(color))
                            .with_filter(default_filter),
                    )
                    .init();
                return warning;
            }
            TracingSubscriber::WithOpenTelemetry(layered) => {
                let (default_filter, warning) = new_log_filter();
                layered
                    .with(
                        tracing_subscriber::fmt::layer()
                            .with_writer(io::stderr as fn() -> io::Stderr)
                            .with_ansi(color)
                            .fmt_fields(console_field_formatter())
                            .with_span_events(FmtSpan::CLOSE)
                            .event_format(CborConsoleEventFormat::new().with_ansi(color))
                            .with_filter(default_filter),
                    )
                    .init();
                return warning;
            }
            TracingSubscriber::WithLocalTelemetry(layered) => {
                layered.init();
            }
            TracingSubscriber::WithLocalTelemetryAndOpenTelemetry(layered) => {
                layered.init();
            }
            TracingSubscriber::WithJson(layered) => {
                layered.init();
            }
            TracingSubscriber::WithJsonAndOpenTelemetry(layered) => {
                layered.init();
            }
        }

        None
    }
}

// -----------------------------------------------------------------------------
// JSON TRACES
// ---------------------------------------------------------------------------------

pub fn setup_json_traces(subscriber: &mut TracingSubscriber<Registry>) -> DelayedWarning {
    let events = || FmtSpan::ENTER | FmtSpan::EXIT;
    let filter = || new_trace_filter();

    subscriber.with_json(
        || {
            let (default_filter, warning) = filter();
            (
                tracing_subscriber::fmt::layer()
                    .with_span_events(events())
                    .event_format(CborJsonEventFormat::new())
                    .fmt_fields(CborJsonFields::new())
                    .with_filter(default_filter),
                warning,
            )
        },
        || {
            let (default_filter, warning) = filter();
            (
                tracing_subscriber::fmt::layer()
                    .with_span_events(events())
                    .event_format(CborJsonEventFormat::new())
                    .fmt_fields(CborJsonFields::new())
                    .with_filter(default_filter),
                warning,
            )
        },
    )
}

// -----------------------------------------------------------------------------
// OPEN TELEMETRY
// -----------------------------------------------------------------------------

pub struct OpenTelemetryHandle {
    pub meter: Meter,
    pub teardown: Box<dyn FnOnce() -> anyhow::Result<()> + Send>,
}

impl Default for OpenTelemetryHandle {
    fn default() -> Self {
        OpenTelemetryHandle {
            meter: Meter::default(),
            teardown: Box::new(|| Ok(())) as Box<dyn FnOnce() -> anyhow::Result<()> + Send>,
        }
    }
}

const DEFAULT_OTLP_SERVICE_NAME: &str = "amaru";

/// Context hints supplied by the caller to refine observability defaults.
pub trait ObservabilityHints {
    /// The address the node will listen on, if known at this point.
    /// Used to build the default `service.instance.id` resource attribute.
    fn listen_address(&self) -> Option<&str>;
}

pub fn new_resource(hints: &impl ObservabilityHints) -> Resource {
    // Build the SDK-default resource to discover attributes already set via
    // OTEL_RESOURCE_ATTRIBUTES. This is used only to guard our *fallback* values;
    // the dedicated OTEL_SERVICE_NAME / OTEL_SERVICE_INSTANCE_ID env vars always
    // take priority and are never suppressed by OTEL_RESOURCE_ATTRIBUTES.
    let explicit_service_name = var("OTEL_SERVICE_NAME").ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let service_name = explicit_service_name.clone().unwrap_or_else(|| DEFAULT_OTLP_SERVICE_NAME.to_string());

    let explicit_service_instance_id =
        var("OTEL_SERVICE_INSTANCE_ID").ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());

    let service_instance_id: Option<String> = explicit_service_instance_id.clone().or_else(|| {
        let listen_addr = hints.listen_address()?;
        let hostname = sysinfo::System::host_name().unwrap_or_else(|| "localhost".to_string());
        let port = listen_addr.trim().rsplit(':').next()?;
        Some(format!("{hostname}:{port}"))
    });

    let mut attributes = Vec::new();
    attributes.push(KeyValue::new(SERVICE_NAME, service_name.clone()));
    if let Some(instance_id) = service_instance_id {
        attributes.push(KeyValue::new(SERVICE_INSTANCE_ID, instance_id));
    }

    Resource::builder().with_attributes(attributes).build()
}

#[expect(clippy::panic)]
pub fn setup_open_telemetry(
    subscriber: &mut TracingSubscriber<Registry>,
    resource: Resource,
) -> (OpenTelemetryHandle, DelayedWarning) {
    let signals = OpenTelemetrySignals::from_env()
        .unwrap_or_else(|error| panic!("failed to configure OpenTelemetry signals: {error}"));
    setup_open_telemetry_with_signals(subscriber, resource, signals)
}

#[expect(clippy::panic)]
#[expect(clippy::expect_used)]
pub fn setup_open_telemetry_with_signals(
    subscriber: &mut TracingSubscriber<Registry>,
    resource: Resource,
    signals: OpenTelemetrySignals,
) -> (OpenTelemetryHandle, DelayedWarning) {
    let service_name = resource
        .get(&Key::from(SERVICE_NAME))
        .expect("missing 'service_name' on the provided OTLP resource")
        .as_str()
        .to_string();

    let traces_provider = signals.traces().then(|| {
        SdkTracerProvider::builder()
            .with_resource(resource.clone())
            .with_batch_exporter(
                opentelemetry_otlp::SpanExporter::builder()
                    .with_tonic()
                    .build()
                    .unwrap_or_else(|e| panic!("failed to setup opentelemetry span exporter: {e}")),
            )
            .build()
    });

    let logs_provider = signals.logs().then(|| {
        SdkLoggerProvider::builder()
            .with_resource(resource.clone())
            .with_batch_exporter(
                opentelemetry_otlp::LogExporter::builder()
                    .with_tonic()
                    .build()
                    .unwrap_or_else(|e| panic!("failed to setup opentelemetry log exporter: {e}")),
            )
            .build()
    });

    let (meter, meter_provider) = if signals.metrics() {
        let metric_exporter = opentelemetry_otlp::MetricExporter::builder()
            .with_tonic()
            .with_temporality(Temporality::default())
            .build()
            .unwrap_or_else(|e| panic!("unable to create metric exporter: {e:?}"));

        let provider = SdkMeterProvider::builder()
            .with_resource(resource.clone())
            .with_reader(opentelemetry_sdk::metrics::PeriodicReader::builder(metric_exporter).build())
            .build();
        (Meter::from(provider.meter(METRICS_METER_NAME)), Some(provider))
    } else {
        (Meter::default(), None)
    };

    let (opentelemetry_layer, warning) = if let Some(provider) = traces_provider.as_ref() {
        let (filter, warning) = new_trace_filter();
        let layer = tracing_opentelemetry::layer()
            .with_tracer(provider.tracer(service_name))
            .with_level(true)
            .with_target(true)
            .with_filter(filter);
        (Some(layer), warning)
    } else {
        (None, None)
    };

    // Logs: project-owned bridge so CBOR field payloads become nested AnyValue maps/lists.
    let (log_bridge, logs_warning) = if let Some(provider) = logs_provider.as_ref() {
        let (filter, warning) = new_trace_filter();
        (Some(CborOtelLogBridge::new(provider).with_filter(filter)), warning)
    } else {
        (None, None)
    };

    subscriber.with_open_telemetry(opentelemetry_layer, log_bridge);

    (
        OpenTelemetryHandle {
            meter,
            teardown: Box::new(|| teardown_open_telemetry(traces_provider, meter_provider, logs_provider)),
        },
        warning.or(logs_warning),
    )
}

fn teardown_open_telemetry(
    tracing: Option<SdkTracerProvider>,
    meter: Option<SdkMeterProvider>,
    logs: Option<SdkLoggerProvider>,
) -> anyhow::Result<()> {
    // Shut down the providers so that it flushes any remaining spans.
    // The process lifecycle layer applies an outer timeout around this teardown.
    use anyhow::Context as _;

    if let Some(provider) = tracing {
        provider.shutdown().context("failed to shutdown OpenTelemetry tracer provider")?;
    }
    if let Some(provider) = meter {
        provider.shutdown().context("failed to shutdown OpenTelemetry meter provider")?;
    }
    if let Some(provider) = logs {
        provider.shutdown().context("failed to shutdown OpenTelemetry log provider")?;
    }

    Ok(())
}

// -----------------------------------------------------------------------------
// ENV FILTER
// -----------------------------------------------------------------------------

/// Wraps an [`EnvFilter`] and replaces repeated OpenTelemetry SDK errors with
/// one actionable Amaru warning.
pub struct OtelErrorFilter {
    inner: EnvFilter,
    reporter: OtelExportReporter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OtelSignal {
    Metrics,
    Traces,
    Logs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OtelExportTransition {
    Failed(OtelSignal),
    Recovered(OtelSignal),
}

impl OtelSignal {
    const ALL: [Self; 3] = [Self::Metrics, Self::Traces, Self::Logs];

    fn as_str(self) -> &'static str {
        match self {
            Self::Metrics => "metrics",
            Self::Traces => "traces",
            Self::Logs => "logs",
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Metrics => 0,
            Self::Traces => 1,
            Self::Logs => 2,
        }
    }
}

impl OtelErrorFilter {
    fn new(inner: EnvFilter, reporter: OtelExportReporter) -> Self {
        Self { inner, reporter }
    }

    fn is_otel_export_error(meta: &Metadata<'_>) -> bool {
        meta.level() == &amaru_observability::tracing::Level::ERROR
            && meta.target().starts_with("opentelemetry")
            && (meta.name().ends_with("ExportError") || meta.name().ends_with("ExportFailed"))
    }

    fn is_otel_export_success(meta: &Metadata<'_>) -> bool {
        meta.level() == &amaru_observability::tracing::Level::DEBUG
            && meta.target().starts_with("opentelemetry")
            && meta.name().ends_with("ExportSucceeded")
    }

    fn otel_signal(meta: &Metadata<'_>) -> OtelSignal {
        if meta.name().contains("Log") {
            OtelSignal::Logs
        } else if meta.name().contains("Span") || meta.name().contains("Trace") {
            OtelSignal::Traces
        } else {
            OtelSignal::Metrics
        }
    }

    fn report_otel_export_failure(&self, meta: &Metadata<'_>) {
        self.reporter.report(OtelExportTransition::Failed(Self::otel_signal(meta)));
    }

    fn report_otel_export_recovery(&self, meta: &Metadata<'_>) {
        self.reporter.report(OtelExportTransition::Recovered(Self::otel_signal(meta)));
    }
}

#[derive(Debug, Clone)]
struct OtelExportReporter {
    /// The reporter thread owns all mutable transition state; filters only send observations.
    sender: mpsc::Sender<OtelExportTransition>,
}

impl OtelExportReporter {
    fn start() -> Self {
        let (sender, receiver) = mpsc::channel();
        let _reporter_thread = std::thread::Builder::new()
            .name("amaru-otel-reporter".into())
            .spawn(move || run_otel_export_reporter(receiver));
        Self { sender }
    }

    fn report(&self, transition: OtelExportTransition) {
        let _ = self.sender.send(transition);
    }
}

fn run_otel_export_reporter(receiver: mpsc::Receiver<OtelExportTransition>) {
    let mut current_failures = [false; 3];
    let mut reported_failures = [false; 3];
    let mut deadline: Option<Instant> = None;

    loop {
        let received = match deadline {
            Some(deadline) => receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())),
            None => match receiver.recv() {
                Ok(transition) => Ok(transition),
                Err(_) => break,
            },
        };

        match received {
            Ok(OtelExportTransition::Failed(signal)) => {
                current_failures[signal.index()] = true;
            }
            Ok(OtelExportTransition::Recovered(signal)) => {
                current_failures[signal.index()] = false;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        if current_failures == reported_failures {
            deadline = None;
            continue;
        }

        let now = Instant::now();
        let Some(report_at) = deadline else {
            deadline = Some(now + OTEL_EXPORT_TRANSITION_DELAY);
            continue;
        };
        if report_at > now {
            continue;
        }

        let (failures, recoveries) = otel_export_changes(&current_failures, &reported_failures);
        reported_failures = current_failures;
        deadline = None;

        if !failures.is_empty() {
            let unavailable_signals = otlp_signals(&failures);
            warn!(setup::observability::EXPORT_FAILED, unavailable_signals);
        }
        if !recoveries.is_empty() {
            let recovered_signals = otlp_signals(&recoveries);
            info!(setup::observability::EXPORT_RECOVERED, recovered_signals);
        }
    }
}

fn otel_export_changes(current: &[bool; 3], reported: &[bool; 3]) -> (Vec<OtelSignal>, Vec<OtelSignal>) {
    let has_new_failure =
        OtelSignal::ALL.into_iter().any(|signal| current[signal.index()] && !reported[signal.index()]);
    let failures = if has_new_failure {
        OtelSignal::ALL.into_iter().filter(|signal| current[signal.index()]).collect()
    } else {
        Vec::new()
    };
    let recoveries =
        OtelSignal::ALL.into_iter().filter(|signal| !current[signal.index()] && reported[signal.index()]).collect();
    (failures, recoveries)
}

fn otlp_signals(signals: &[OtelSignal]) -> String {
    signals.iter().map(|signal| signal.as_str()).collect::<Vec<_>>().join(",")
}

impl<S: Subscriber> Filter<S> for OtelErrorFilter {
    fn enabled(&self, meta: &Metadata<'_>, cx: &Context<'_, S>) -> bool {
        if Self::is_otel_export_error(meta) {
            self.report_otel_export_failure(meta);
            return false;
        }
        if Self::is_otel_export_success(meta) {
            self.report_otel_export_recovery(meta);
            return false;
        }
        if !<EnvFilter as Filter<S>>::enabled(&self.inner, meta, cx) {
            return false;
        }
        true
    }

    fn callsite_enabled(&self, meta: &'static Metadata<'static>) -> Interest {
        // Force per-call evaluation so export state transitions cannot bypass
        // the replacement messages through callsite caching.
        if Self::is_otel_export_error(meta) || Self::is_otel_export_success(meta) {
            return Interest::sometimes();
        }
        <EnvFilter as Filter<S>>::callsite_enabled(&self.inner, meta)
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        <EnvFilter as Filter<S>>::max_level_hint(&self.inner).map(|level| level.max(LevelFilter::DEBUG))
    }

    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        <EnvFilter as Filter<S>>::on_new_span(&self.inner, attrs, id, ctx);
    }

    fn on_record(&self, id: &span::Id, values: &span::Record<'_>, ctx: Context<'_, S>) {
        <EnvFilter as Filter<S>>::on_record(&self.inner, id, values, ctx);
    }

    fn on_enter(&self, id: &span::Id, ctx: Context<'_, S>) {
        <EnvFilter as Filter<S>>::on_enter(&self.inner, id, ctx);
    }

    fn on_exit(&self, id: &span::Id, ctx: Context<'_, S>) {
        <EnvFilter as Filter<S>>::on_exit(&self.inner, id, ctx);
    }

    fn on_close(&self, id: span::Id, ctx: Context<'_, S>) {
        <EnvFilter as Filter<S>>::on_close(&self.inner, id, ctx);
    }
}

fn new_default_filter(var: &str, default: &str) -> (OtelErrorFilter, DelayedWarning) {
    let (filter, warning) = match EnvFilter::try_from_env(var) {
        Ok(filter) => {
            let var = var.to_string();
            let value = std::env::var(&var).unwrap_or_default();
            let notice = Box::new(move || {
                info!(setup::trace::FILTER, var, value, provided_by_user = true);
            }) as Box<dyn FnOnce()>;
            (filter, Some(notice))
        }
        Err(e) => {
            // Notice stashed for when the tracing system is up.
            let fallback = default.to_string();
            let var = var.to_string();
            let warning = match e.source().and_then(|e| e.downcast_ref::<VarError>()) {
                Some(VarError::NotPresent) => Box::new(move || {
                    info!(setup::trace::FILTER, var, value = fallback, provided_by_user = false);
                }) as Box<dyn FnOnce()>,
                _ => Box::new(move || {
                    warn!(
                        setup::trace::FILTER,
                        var,
                        value = fallback,
                        provided_by_user = true,
                        provided_invalid = true,
                        error = e.to_string()
                    );
                }) as Box<dyn FnOnce()>,
            };

            #[expect(clippy::expect_used)]
            let filter = EnvFilter::try_new(default).expect("invalid default filter");
            (filter, Some(warning))
        }
    };
    let reporter = OTEL_EXPORT_REPORTER.get_or_init(OtelExportReporter::start);
    (OtelErrorFilter::new(filter, reporter.clone()), warning)
}

fn new_log_filter() -> (OtelErrorFilter, DelayedWarning) {
    new_default_filter(AMARU_LOG_VAR, DEFAULT_AMARU_LOG_FILTER)
}

fn new_trace_filter() -> (OtelErrorFilter, DelayedWarning) {
    new_default_filter(AMARU_TRACE_VAR, DEFAULT_AMARU_TRACE_FILTER)
}

/// Optional in-process telemetry sinks supplied by the product binary or an embedder
/// (TUI, custom dashboards, test harnesses). Types intentionally avoid naming `amaru-tui`.
pub struct LocalTelemetry {
    pub metrics_observer: Option<Box<dyn Fn(&amaru_metrics::MetricsEvent) + Send + Sync>>,
    pub capture_layer: Option<TelemetryCaptureLayer>,
}

#[expect(clippy::panic)]
pub fn setup_observability(
    with_open_telemetry: bool,
    with_json_traces: bool,
    local: Option<LocalTelemetry>,
    color: bool,
    hints: &impl ObservabilityHints,
) -> OpenTelemetryHandle {
    try_setup_observability(with_open_telemetry, with_json_traces, local, color, hints)
        .unwrap_or_else(|error| panic!("failed to configure observability: {error}"))
}

pub fn try_setup_observability(
    with_open_telemetry: bool,
    with_json_traces: bool,
    local: Option<LocalTelemetry>,
    color: bool,
    hints: &impl ObservabilityHints,
) -> anyhow::Result<OpenTelemetryHandle> {
    let mut subscriber = TracingSubscriber::new();

    let (OpenTelemetryHandle { mut meter, teardown }, warning_otlp) = if with_open_telemetry {
        let signals = OpenTelemetrySignals::from_env()?;
        setup_open_telemetry_with_signals(&mut subscriber, new_resource(hints), signals)
    } else {
        (OpenTelemetryHandle::default(), None)
    };

    let warning_local = if let Some(local) = local {
        if let Some(observer) = local.metrics_observer {
            meter.set_local_observer(observer);
        }
        if let Some(layer) = local.capture_layer { subscriber.with_local_telemetry(layer) } else { None }
    } else {
        None
    };

    let warning_json = if with_json_traces { setup_json_traces(&mut subscriber) } else { None };

    let warning_log = subscriber.init(color);

    for notify in [warning_otlp, warning_local, warning_json, warning_log].into_iter().flatten() {
        notify();
    }

    info!(setup::observability::INIT, with_open_telemetry, with_json_traces, with_colors = color,);

    Ok(OpenTelemetryHandle { meter, teardown })
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

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering as AtomicOrdering},
    };

    use amaru_observability::tracing;

    use super::*;

    #[test]
    fn otel_export_error_is_recognised() {
        // Use the actual tracing machinery to produce `Metadata` with a known
        // target, level, and operation name, then classify it.
        static CHECK: Mutex<Option<bool>> = Mutex::new(None);

        struct CaptureMeta;
        impl tracing::Subscriber for CaptureMeta {
            fn enabled(&self, meta: &tracing::Metadata<'_>) -> bool {
                if meta.name() == "BatchLogProcessor.ExportError" {
                    *CHECK.lock().unwrap() = Some(OtelErrorFilter::is_otel_export_error(meta));
                }
                true
            }
            fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
                tracing::span::Id::from_u64(1)
            }
            fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
            fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
            fn event(&self, _: &tracing::Event<'_>) {}
            fn enter(&self, _: &tracing::span::Id) {}
            fn exit(&self, _: &tracing::span::Id) {}
        }

        tracing::subscriber::with_default(CaptureMeta, || {
            tracing::event!(name: "BatchLogProcessor.ExportError", target: "opentelemetry_sdk", tracing::Level::ERROR, "test");
        });

        assert_eq!(*CHECK.lock().unwrap(), Some(true));
    }

    #[test]
    fn non_otel_target_is_not_recognised_as_export_error() {
        static CHECK: Mutex<Option<bool>> = Mutex::new(None);

        struct CaptureMeta;
        impl tracing::Subscriber for CaptureMeta {
            fn enabled(&self, meta: &tracing::Metadata<'_>) -> bool {
                if meta.target() == "amaru::stages" {
                    *CHECK.lock().unwrap() = Some(OtelErrorFilter::is_otel_export_error(meta));
                }
                true
            }
            fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
                tracing::span::Id::from_u64(1)
            }
            fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
            fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
            fn event(&self, _: &tracing::Event<'_>) {}
            fn enter(&self, _: &tracing::span::Id) {}
            fn exit(&self, _: &tracing::span::Id) {}
        }

        tracing::subscriber::with_default(CaptureMeta, || {
            tracing::event!(target: "amaru::stages", tracing::Level::DEBUG, "test");
        });

        assert_eq!(*CHECK.lock().unwrap(), Some(false));
    }

    #[test]
    fn otel_errors_are_suppressed_after_scheduling_one_warning() {
        let (reporter, transitions) = test_reporter();
        let filter = OtelErrorFilter::new(EnvFilter::try_new("error").unwrap(), reporter);
        let seen = count_events(filter, || {
            tracing::event!(name: "BatchLogProcessor.ExportError", target: "opentelemetry_sdk", tracing::Level::ERROR, "first");
            tracing::event!(name: "BatchSpanProcessor.ExportError", target: "opentelemetry_sdk", tracing::Level::ERROR, "second");
        });
        assert_eq!(seen, 0);
        assert_eq!(transitions.recv().unwrap(), OtelExportTransition::Failed(OtelSignal::Logs));
        assert_eq!(transitions.recv().unwrap(), OtelExportTransition::Failed(OtelSignal::Traces));
    }

    #[test]
    fn successful_exports_batch_recovery_and_allow_a_later_failure_warning() {
        let available = [false; 3];
        let traces_and_logs_unavailable = [false, true, true];

        assert_eq!(
            otel_export_changes(&traces_and_logs_unavailable, &available),
            (vec![OtelSignal::Traces, OtelSignal::Logs], vec![])
        );
        assert_eq!(
            otel_export_changes(&available, &traces_and_logs_unavailable),
            (vec![], vec![OtelSignal::Traces, OtelSignal::Logs])
        );
        assert_eq!(otel_export_changes(&[false, false, true], &available), (vec![OtelSignal::Logs], vec![]));
    }

    #[test]
    fn non_error_otel_event_is_not_suppressed() {
        let filter = OtelErrorFilter::new(EnvFilter::try_new("debug").unwrap(), test_reporter().0);
        let seen = count_events(filter, || {
            tracing::event!(name: "BatchLogProcessor.ExportError", target: "opentelemetry_sdk", tracing::Level::WARN, "test");
        });
        assert_eq!(seen, 1);
    }

    #[test]
    fn unrelated_otel_error_is_not_suppressed() {
        let filter = OtelErrorFilter::new(EnvFilter::try_new("error").unwrap(), test_reporter().0);
        let seen = count_events(filter, || {
            tracing::event!(name: "TracerProvider.GlobalSetFailed", target: "opentelemetry", tracing::Level::ERROR, "test");
        });
        assert_eq!(seen, 1);
    }

    #[test]
    fn non_otel_error_is_not_suppressed() {
        let filter = OtelErrorFilter::new(EnvFilter::try_new("error").unwrap(), test_reporter().0);
        let seen = count_events(filter, || {
            tracing::error!(target: "amaru::stages", "test");
        });
        assert_eq!(seen, 1);
    }

    // HELPERS

    fn test_reporter() -> (OtelExportReporter, mpsc::Receiver<OtelExportTransition>) {
        let (sender, receiver) = mpsc::channel();
        (OtelExportReporter { sender }, receiver)
    }

    /// Builds a subscriber that wraps an `OtelErrorFilter` and counts how
    /// many events pass the filter.  Installing it as the default inside a
    /// closure keeps tests independent even when run in parallel.
    struct CountingLayer {
        count: Arc<AtomicUsize>,
    }

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CountingLayer {
        fn on_event(&self, _event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
            self.count.fetch_add(1, AtomicOrdering::Relaxed);
        }
    }

    /// Runs `f` with a subscriber that applies `filter` and returns the number
    /// of events that were seen by the inner layer.
    fn count_events<F: FnOnce()>(filter: OtelErrorFilter, f: F) -> usize {
        let count = Arc::new(AtomicUsize::new(0));
        let subscriber =
            tracing_subscriber::registry().with(CountingLayer { count: Arc::clone(&count) }.with_filter(filter));
        tracing::subscriber::with_default(subscriber, f);
        count.load(AtomicOrdering::Relaxed)
    }
}
