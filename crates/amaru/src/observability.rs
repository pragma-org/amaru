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
    fmt,
    io::{self, IsTerminal},
    str::FromStr,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use amaru_metrics::{METRICS_METER_NAME, Meter};
use amaru_observability::{TelemetryCaptureLayer, info, warn};
use opentelemetry::{Key, KeyValue, metrics::MeterProvider, trace::TracerProvider};
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_sdk::{
    Resource,
    logs::SdkLoggerProvider,
    metrics::{SdkMeterProvider, Temporality},
    trace::SdkTracerProvider,
};
use opentelemetry_semantic_conventions::resource::{SERVICE_INSTANCE_ID, SERVICE_NAME};
use tracing::{
    Metadata, Subscriber,
    field::{Field, Visit},
    level_filters::LevelFilter,
    span,
    subscriber::Interest,
};
use tracing_subscriber::{
    EnvFilter, Registry,
    field::{MakeVisitor, RecordFields, VisitOutput},
    filter::Filtered,
    fmt::{
        FmtContext, FormatEvent, FormatFields, FormattedFields, Layer,
        format::{DefaultFields, FmtSpan, Format, Json, JsonFields, JsonVisitor, Writer},
    },
    layer::{Context, Filter, Layered, SubscriberExt},
    prelude::*,
    registry::LookupSpan,
    util::SubscriberInitExt,
};

const AMARU_LOG_VAR: &str = "AMARU_LOG";

const DEFAULT_AMARU_LOG_FILTER: &str = "error,amaru=info,amaru_pure_stage=warn";

const AMARU_TRACE_VAR: &str = "AMARU_TRACE";

const DEFAULT_AMARU_TRACE_FILTER: &str = "error,amaru=debug,amaru_pure_stage=warn";

const OTEL_ERROR_THROTTLE_MS: u64 = 5_000;

// -----------------------------------------------------------------------------
// TracingSubscriber
// -----------------------------------------------------------------------------

type InnerOtelStack<S> = Layered<OpenTelemetryFilter<S>, S>;

type OpenTelemetryLayer<S> = Layered<LogBridgeFilter<S>, InnerOtelStack<S>>;

type LogBridgeFilter<S> = Filtered<
    OpenTelemetryTracingBridge<SdkLoggerProvider, opentelemetry_sdk::logs::SdkLogger>,
    ThrottledEnvFilter,
    InnerOtelStack<S>,
>;

type OpenTelemetryFilter<S> =
    Filtered<tracing_opentelemetry::OpenTelemetryLayer<S, opentelemetry_sdk::trace::Tracer>, ThrottledEnvFilter, S>;

type JsonLayer<S> = Layered<JsonFilter<S>, S>;

type JsonFilter<S> = Filtered<Layer<S, HideTagFields<JsonFields>, SpanJsonFormat>, ThrottledEnvFilter, S>;

type LocalTelemetryFilter<S> = Filtered<TelemetryCaptureLayer, ThrottledEnvFilter, S>;

type LocalTelemetryLayer<S> = Layered<LocalTelemetryFilter<S>, S>;

type DelayedWarning = Option<Box<dyn FnOnce()>>;

// -----------------------------------------------------------------------------
// HideTagFields
//
// The `amaru.tag.<name>` boolean attributes categorize spans (cpu/io/setup) for
// OpenTelemetry backends. They carry no value in serialized logs, and because
// formatters append the fields of spans in scope, an inherited tag would otherwise
// repeat once per nested span. This field formatter drops them from console and
// JSON output while leaving OpenTelemetry spans untouched.
// -----------------------------------------------------------------------------

const TAG_FIELD_PREFIX: &str = "amaru.tag.";

fn is_tag_field(field: &Field) -> bool {
    is_tag_field_name(field.name())
}

fn is_tag_field_name(name: &str) -> bool {
    name.starts_with(TAG_FIELD_PREFIX)
}

/// Wraps a field formatter so that `amaru.tag.*` fields are skipped.
pub struct HideTagFields<N>(N);

impl<'writer> FormatFields<'writer> for HideTagFields<DefaultFields> {
    fn format_fields<R: RecordFields>(&self, writer: Writer<'writer>, fields: R) -> fmt::Result {
        format_without_tag_fields(fields, self.0.make_visitor(writer))
    }
}

/// Forwards every recorded value to the inner visitor except `amaru.tag.*`
/// fields, which are dropped. Each typed method is forwarded individually so the
/// inner visitor keeps its per-type formatting.
struct HideTagVisitor<V>(V);

impl<V: Visit> Visit for HideTagVisitor<V> {
    fn record_f64(&mut self, field: &Field, value: f64) {
        if !is_tag_field(field) {
            self.0.record_f64(field, value);
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        if !is_tag_field(field) {
            self.0.record_i64(field, value);
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        if !is_tag_field(field) {
            self.0.record_u64(field, value);
        }
    }

    fn record_i128(&mut self, field: &Field, value: i128) {
        if !is_tag_field(field) {
            self.0.record_i128(field, value);
        }
    }

    fn record_u128(&mut self, field: &Field, value: u128) {
        if !is_tag_field(field) {
            self.0.record_u128(field, value);
        }
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        if !is_tag_field(field) {
            self.0.record_bool(field, value);
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if !is_tag_field(field) {
            self.0.record_str(field, value);
        }
    }

    fn record_bytes(&mut self, field: &Field, value: &[u8]) {
        if !is_tag_field(field) {
            self.0.record_bytes(field, value);
        }
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        if !is_tag_field(field) {
            self.0.record_error(field, value);
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if !is_tag_field(field) {
            self.0.record_debug(field, value);
        }
    }
}

impl<Out, V: VisitOutput<Out>> VisitOutput<Out> for HideTagVisitor<V> {
    fn finish(self) -> Out {
        self.0.finish()
    }
}

impl<'writer> FormatFields<'writer> for HideTagFields<JsonFields> {
    fn format_fields<R: RecordFields>(&self, mut writer: Writer<'writer>, fields: R) -> fmt::Result {
        // JsonFields does not expose a visitor factory, so apply the shared
        // filtering visitor to the same JsonVisitor it uses internally.
        format_without_tag_fields(fields, JsonVisitor::new(&mut writer))
    }

    fn add_fields(&self, current: &'writer mut FormattedFields<Self>, fields: &span::Record<'_>) -> fmt::Result {
        // Preserve JsonFields' object-merging behavior instead of duplicating it.
        let mut formatted = FormattedFields::<JsonFields>::new(std::mem::take(&mut current.fields));
        let result =
            self.0.add_fields(&mut formatted, fields).and_then(|_| remove_json_tag_fields(&mut formatted.fields));
        current.fields = formatted.fields;
        result
    }
}

fn format_without_tag_fields(fields: impl RecordFields, visitor: impl VisitOutput<fmt::Result>) -> fmt::Result {
    let mut visitor = HideTagVisitor(visitor);
    fields.record(&mut visitor);
    visitor.finish()
}

fn remove_json_tag_fields(formatted: &mut String) -> fmt::Result {
    if !formatted.contains(TAG_FIELD_PREFIX) {
        return Ok(());
    }

    let mut fields: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(formatted).map_err(|_| fmt::Error)?;
    fields.retain(|name, _| !is_tag_field_name(name));
    *formatted = serde_json::to_string(&fields).map_err(|_| fmt::Error)?;
    Ok(())
}

// -----------------------------------------------------------------------------
// SpanJsonFormat
//
// Wraps the standard JSON formatter to inject `id` and `parent_id` top-level
// fields into span lifecycle events (enter/exit). Regular log events are left
// untouched.
// -----------------------------------------------------------------------------

pub struct SpanJsonFormat(Format<Json>);

impl<S, N> FormatEvent<S, N> for SpanJsonFormat
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> fmt::Result {
        // Render the event with the inner JSON formatter into a buffer
        let mut buf = String::new();
        self.0.format_event(ctx, Writer::new(&mut buf), event)?;

        // Inject span-related fields before the closing '}'.
        //  - Span lifecycle events (enter/exit): get `id`, `parent_id`, and recorded fields.
        //  - Log events emitted inside a span: get `parent_id` only.
        if let Some(current) = ctx.lookup_current()
            && let Some(pos) = buf.rfind('}')
        {
            let mut extra = String::new();

            // Inject recorded span fields (stored by the fmt layer as FormattedFields).
            let extensions = current.extensions();
            if let Some(fields) = extensions.get::<FormattedFields<N>>() {
                let s = fields.as_str().trim();
                // Strip outer braces from JSON object: {"k":v,...} -> "k":v,...
                let inner = s.strip_prefix('{').and_then(|s| s.strip_suffix('}')).unwrap_or(s);
                if !inner.is_empty() {
                    extra.push(',');
                    extra.push_str(inner);
                }
            }

            if event.metadata().is_span() {
                let id = current.id().into_u64();
                extra.push_str(&format!(",\"id\":{id}"));
            }
            if let Some(parent) = current.parent() {
                let parent_id = parent.id().into_u64();
                extra.push_str(&format!(",\"parent_id\":{parent_id}"));
            }
            if !extra.is_empty() {
                buf.insert_str(pos, &extra);
            }
        }

        writer.write_str(&buf)
    }
}

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
    pub fn with_open_telemetry(&mut self, layer: OpenTelemetryFilter<Registry>, log_bridge: LogBridgeFilter<Registry>) {
        match std::mem::take(self) {
            Self::Registry(registry) => {
                *self = TracingSubscriber::WithOpenTelemetry(registry.with(layer).with(log_bridge));
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
                *self = TracingSubscriber::WithJson(registry.with(layer));
                warning
            }
            Self::WithOpenTelemetry(layered) => {
                let (layer, warning) = otel_layer();
                *self = TracingSubscriber::WithJsonAndOpenTelemetry(layered.with(layer));
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
                            .fmt_fields(HideTagFields(DefaultFields::new()))
                            .event_format(tracing_subscriber::fmt::format().with_ansi(color).compact())
                            .with_span_events(FmtSpan::CLOSE)
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
                            .fmt_fields(HideTagFields(DefaultFields::new()))
                            .event_format(tracing_subscriber::fmt::format().with_ansi(color).compact())
                            .with_span_events(FmtSpan::CLOSE)
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
    let format = || SpanJsonFormat(tracing_subscriber::fmt::format().json().with_span_list(false));
    let events = || FmtSpan::ENTER | FmtSpan::EXIT;
    let filter = || new_trace_filter();

    subscriber.with_json(
        || {
            let (default_filter, warning) = filter();
            (
                tracing_subscriber::fmt::layer()
                    .with_span_events(events())
                    .event_format(format())
                    .fmt_fields(HideTagFields(JsonFields::new()))
                    .with_filter(default_filter),
                warning,
            )
        },
        || {
            let (default_filter, warning) = filter();
            (
                tracing_subscriber::fmt::layer()
                    .with_span_events(events())
                    .event_format(format())
                    .fmt_fields(HideTagFields(JsonFields::new()))
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
    pub teardown: Box<dyn FnOnce() -> Result<(), Box<dyn std::error::Error>> + Send>,
}

impl Default for OpenTelemetryHandle {
    fn default() -> Self {
        OpenTelemetryHandle {
            meter: Meter::default(),
            teardown: Box::new(|| Ok(())) as Box<dyn FnOnce() -> Result<(), Box<dyn std::error::Error>> + Send>,
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
#[expect(clippy::expect_used)]
pub fn setup_open_telemetry(
    subscriber: &mut TracingSubscriber<Registry>,
    resource: Resource,
) -> (OpenTelemetryHandle, DelayedWarning) {
    let service_name = resource
        .get(&Key::from(SERVICE_NAME))
        .expect("missing 'service_name' on the provided OTLP resource")
        .as_str()
        .to_string();

    let traces_provider = SdkTracerProvider::builder()
        .with_resource(resource.clone())
        .with_batch_exporter(
            opentelemetry_otlp::SpanExporter::builder()
                .with_tonic()
                .build()
                .unwrap_or_else(|e| panic!("failed to setup opentelemetry span exporter: {e}")),
        )
        .build();

    let logs_provider = SdkLoggerProvider::builder()
        .with_resource(resource.clone())
        .with_batch_exporter(
            opentelemetry_otlp::LogExporter::builder()
                .with_tonic()
                .build()
                .unwrap_or_else(|e| panic!("failed to setup opentelemetry log exporter: {e}")),
        )
        .build();

    let metric_exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_tonic()
        .with_temporality(Temporality::default())
        .build()
        .unwrap_or_else(|e| panic!("unable to create metric exporter: {e:?}"));

    let meter_provider = SdkMeterProvider::builder()
        .with_resource(resource.clone())
        .with_reader(opentelemetry_sdk::metrics::PeriodicReader::builder(metric_exporter).build())
        .build();

    // Subscriber
    let (default_filter, warning) = new_trace_filter();

    let opentelemetry_layer = tracing_opentelemetry::layer()
        .with_tracer(traces_provider.tracer(service_name))
        .with_level(true)
        .with_target(true)
        .with_filter(default_filter);

    // Logs
    let log_bridge = OpenTelemetryTracingBridge::new(&logs_provider).with_filter(new_trace_filter().0);

    subscriber.with_open_telemetry(opentelemetry_layer, log_bridge);

    (
        OpenTelemetryHandle {
            meter: Meter::from(meter_provider.meter(METRICS_METER_NAME)),
            teardown: Box::new(|| teardown_open_telemetry(traces_provider, meter_provider, logs_provider)),
        },
        warning,
    )
}

fn teardown_open_telemetry(
    tracing: SdkTracerProvider,
    meter: SdkMeterProvider,
    logs: SdkLoggerProvider,
) -> Result<(), Box<dyn std::error::Error>> {
    // Shut down the providers so that it flushes any remaining spans.
    // The process lifecycle layer applies an outer timeout around this teardown.
    tracing.shutdown()?;
    meter.shutdown()?;
    logs.shutdown()?;

    Ok(())
}

// -----------------------------------------------------------------------------
// ENV FILTER
// -----------------------------------------------------------------------------

/// Wraps an [`EnvFilter`] and rate-limits events emitted by OpenTelemetry SDK
/// internals (target `opentelemetry*`) to at most one per `throttle_ms`
/// milliseconds. This prevents the console from being flooded with
/// `BatchSpanProcessor.ExportError` messages whenever the OTLP endpoint is
/// temporarily unreachable.
pub struct ThrottledEnvFilter {
    inner: EnvFilter,
    last_otel_event: AtomicU64,
    throttle_ms: u64,
}

impl ThrottledEnvFilter {
    fn new(inner: EnvFilter, throttle_ms: u64) -> Self {
        Self { inner, last_otel_event: AtomicU64::new(0), throttle_ms }
    }

    /// Returns true for events emitted by the OpenTelemetry SDK itself.
    /// These are the ones we want to throttle to avoid log flooding when the
    /// OTLP endpoint is unreachable.
    fn is_otel_internal(meta: &Metadata<'_>) -> bool {
        meta.target().starts_with("opentelemetry")
    }
}

impl<S: Subscriber> Filter<S> for ThrottledEnvFilter {
    fn enabled(&self, meta: &Metadata<'_>, cx: &Context<'_, S>) -> bool {
        if !<EnvFilter as Filter<S>>::enabled(&self.inner, meta, cx) {
            return false;
        }
        if Self::is_otel_internal(meta) {
            // If the system clock is before the Unix epoch, allow the event
            // through rather than freezing throttling forever at timestamp 0.
            let Some(now) = SystemTime::now().duration_since(UNIX_EPOCH).ok().map(|d| d.as_millis() as u64) else {
                return true;
            };
            // Use fetch_update so the read-check-write is one atomic step.
            // A race between threads that both observe an elapsed throttle period
            // may let a small number of extra events through (false positives), but
            // that is acceptable — we only need best-effort throttling here.
            return self
                .last_otel_event
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |last| {
                    (now.saturating_sub(last) >= self.throttle_ms).then_some(now)
                })
                .is_ok();
        }
        true
    }

    fn callsite_enabled(&self, meta: &'static Metadata<'static>) -> Interest {
        // For OTel internal events, force per-call evaluation so that the
        // throttle in `enabled` is never bypassed by callsite caching.
        if Self::is_otel_internal(meta) {
            return Interest::sometimes();
        }
        <EnvFilter as Filter<S>>::callsite_enabled(&self.inner, meta)
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        <EnvFilter as Filter<S>>::max_level_hint(&self.inner)
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

fn new_default_filter(var: &str, default: &str) -> (ThrottledEnvFilter, DelayedWarning) {
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
                    warn!(setup::trace::FILTER, var, value = fallback, provided_by_user = true, provided_invalid = true, error = %e);
                }) as Box<dyn FnOnce()>,
            };

            #[expect(clippy::expect_used)]
            let filter = EnvFilter::try_new(default).expect("invalid default filter");
            (filter, Some(warning))
        }
    };
    (ThrottledEnvFilter::new(filter, OTEL_ERROR_THROTTLE_MS), warning)
}

fn new_log_filter() -> (ThrottledEnvFilter, DelayedWarning) {
    new_default_filter(AMARU_LOG_VAR, DEFAULT_AMARU_LOG_FILTER)
}

fn new_trace_filter() -> (ThrottledEnvFilter, DelayedWarning) {
    new_default_filter(AMARU_TRACE_VAR, DEFAULT_AMARU_TRACE_FILTER)
}

/// Optional in-process telemetry sinks supplied by the product binary or an embedder
/// (TUI, custom dashboards, test harnesses). Types intentionally avoid naming `amaru-tui`.
pub struct LocalTelemetry {
    pub metrics_observer: Option<Box<dyn Fn(&amaru_metrics::MetricsEvent) + Send + Sync>>,
    pub capture_layer: Option<TelemetryCaptureLayer>,
}

pub fn setup_observability(
    with_open_telemetry: bool,
    with_json_traces: bool,
    local: Option<LocalTelemetry>,
    color: bool,
    hints: &impl ObservabilityHints,
) -> OpenTelemetryHandle {
    let mut subscriber = TracingSubscriber::new();

    let (OpenTelemetryHandle { mut meter, teardown }, warning_otlp) = if with_open_telemetry {
        setup_open_telemetry(&mut subscriber, new_resource(hints))
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

    OpenTelemetryHandle { meter, teardown }
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

    use super::*;

    /// The compact console formatter appends the fields of every span in scope to
    /// each event. When several nested spans each carry an `amaru.tag.*` marker
    /// (as all ledger spans do), the tag would otherwise repeat once per span.
    /// `HideTagFields` must strip those markers while keeping ordinary fields.
    #[test]
    fn console_hides_tag_fields_from_nested_spans() {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let layer = tracing_subscriber::fmt::layer()
            .with_writer(BufferWriter(Arc::clone(&buffer)))
            .with_ansi(false)
            .fmt_fields(HideTagFields(DefaultFields::new()))
            .event_format(tracing_subscriber::fmt::format().with_ansi(false).compact());
        let subscriber = tracing_subscriber::registry().with(layer);

        tracing::subscriber::with_default(subscriber, || {
            let outer = tracing::info_span!("outer", "amaru.tag.cpu" = true);
            let _outer = outer.enter();
            let inner = tracing::info_span!("inner", "amaru.tag.cpu" = true, transaction_id = "abc");
            let _inner = inner.enter();
            tracing::info!("hello");
        });

        let output = String::from_utf8(buffer.lock().unwrap().clone()).unwrap();
        assert!(!output.contains("amaru.tag.cpu"), "tag markers must be hidden from the console: {output}");
        assert!(output.contains("transaction_id=\"abc\""), "ordinary span fields must be kept: {output}");
    }

    #[test]
    fn json_hides_tag_fields_from_spans() {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let layer = tracing_subscriber::fmt::layer()
            .with_writer(BufferWriter(Arc::clone(&buffer)))
            .with_span_events(FmtSpan::ENTER | FmtSpan::EXIT)
            .event_format(SpanJsonFormat(tracing_subscriber::fmt::format().json().with_span_list(false)))
            .fmt_fields(HideTagFields(JsonFields::new()));
        let subscriber = tracing_subscriber::registry().with(layer);

        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!(
                "operation",
                "amaru.tag.cpu" = true,
                "amaru.tag.io" = tracing::field::Empty,
                transaction_id = "abc",
                result = tracing::field::Empty,
            );
            span.record("amaru.tag.io", true);
            span.record("result", "ok");
            let _span = span.enter();
            tracing::info!("hello");
        });

        let output = String::from_utf8(buffer.lock().unwrap().clone()).unwrap();
        assert!(!output.is_empty());
        assert!(
            output.lines().all(|line| serde_json::from_str::<serde_json::Value>(line).is_ok()),
            "each output line must be valid JSON: {output}"
        );
        assert!(!output.contains(TAG_FIELD_PREFIX), "tag markers must be hidden from JSON: {output}");
        assert!(output.contains(r#""transaction_id":"abc""#), "initial span fields must be kept: {output}");
        assert!(output.contains(r#""result":"ok""#), "recorded span fields must be kept: {output}");
    }

    #[test]
    fn otel_target_is_recognised_as_internal() {
        // Use the actual tracing machinery to produce `Metadata` with a known
        // target and level, then check `is_otel_internal` on it.
        static CHECK: Mutex<Option<bool>> = Mutex::new(None);

        struct CaptureMeta;
        impl tracing::Subscriber for CaptureMeta {
            fn enabled(&self, meta: &tracing::Metadata<'_>) -> bool {
                if meta.target().starts_with("opentelemetry") {
                    *CHECK.lock().unwrap() = Some(ThrottledEnvFilter::is_otel_internal(meta));
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
            tracing::event!(target: "opentelemetry_sdk::internal", tracing::Level::ERROR, "test");
        });

        assert_eq!(*CHECK.lock().unwrap(), Some(true));
    }

    #[test]
    fn non_otel_target_is_not_recognised_as_internal() {
        static CHECK: Mutex<Option<bool>> = Mutex::new(None);

        struct CaptureMeta;
        impl tracing::Subscriber for CaptureMeta {
            fn enabled(&self, meta: &tracing::Metadata<'_>) -> bool {
                if meta.target() == "amaru::stages" {
                    *CHECK.lock().unwrap() = Some(ThrottledEnvFilter::is_otel_internal(meta));
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
    fn first_otel_event_is_allowed() {
        // last_otel_event starts at 0, so the first event always passes.
        let filter = ThrottledEnvFilter::new(EnvFilter::try_new("error").unwrap(), 1_000);
        let seen = count_events(filter, || {
            tracing::event!(target: "opentelemetry_sdk::internal", tracing::Level::ERROR, "test");
        });
        assert_eq!(seen, 1);
    }

    #[test]
    fn second_otel_event_within_throttle_is_rejected() {
        let filter = ThrottledEnvFilter::new(EnvFilter::try_new("error").unwrap(), 1_000);
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
        // Pre-load `last_otel_event` as if an event was just allowed.
        filter.last_otel_event.store(now, Ordering::Relaxed);

        let seen = count_events(filter, || {
            tracing::event!(target: "opentelemetry_sdk::internal", tracing::Level::ERROR, "test");
        });
        assert_eq!(seen, 0);
    }

    #[test]
    fn otel_event_after_throttle_period_is_allowed() {
        let throttle_ms = 1_000u64;
        let filter = ThrottledEnvFilter::new(EnvFilter::try_new("error").unwrap(), throttle_ms);
        let past = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64 - throttle_ms - 1;
        filter.last_otel_event.store(past, Ordering::Relaxed);

        let seen = count_events(filter, || {
            tracing::event!(target: "opentelemetry_sdk::internal", tracing::Level::ERROR, "test");
        });
        assert_eq!(seen, 1);
    }

    #[test]
    fn non_otel_event_is_not_throttled() {
        let filter = ThrottledEnvFilter::new(EnvFilter::try_new("debug").unwrap(), 1_000);
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
        filter.last_otel_event.store(now, Ordering::Relaxed);

        let seen = count_events(filter, || {
            // Non-otel target — throttle must not apply.
            tracing::debug!(target: "amaru::stages", "test");
        });
        assert_eq!(seen, 1);
    }

    #[test]
    fn throttle_period_advances_after_allowed_event() {
        let filter = ThrottledEnvFilter::new(EnvFilter::try_new("error").unwrap(), 100);
        // Emit two events in rapid succession; only the first should be seen.
        let seen = count_events(filter, || {
            tracing::event!(target: "opentelemetry_sdk::internal", tracing::Level::ERROR, "first");
            tracing::event!(target: "opentelemetry_sdk::internal", tracing::Level::ERROR, "second");
        });
        assert_eq!(seen, 1);
    }

    // HELPERS

    /// A `MakeWriter` that accumulates everything written into a shared buffer,
    /// so a test can inspect the formatted output.
    #[derive(Clone)]
    struct BufferWriter(Arc<Mutex<Vec<u8>>>);

    impl io::Write for BufferWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for BufferWriter {
        type Writer = BufferWriter;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// Builds a subscriber that wraps a `ThrottledEnvFilter` and counts how
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
    fn count_events<F: FnOnce()>(filter: ThrottledEnvFilter, f: F) -> usize {
        let count = Arc::new(AtomicUsize::new(0));
        let subscriber =
            tracing_subscriber::registry().with(CountingLayer { count: Arc::clone(&count) }.with_filter(filter));
        tracing::subscriber::with_default(subscriber, f);
        count.load(AtomicOrdering::Relaxed)
    }
}
