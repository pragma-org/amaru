use miette::IntoDiagnostic;
use opentelemetry_sdk::{metrics::SdkMeterProvider, trace::TracerProvider};
use std::env;
use tracing_subscriber::{
    filter::Filtered,
    fmt::{
        format::{FmtSpan, Format, Json, JsonFields},
        Layer,
    },
    layer::{Layered, SubscriberExt},
    prelude::*,
    util::SubscriberInitExt,
    EnvFilter, Registry,
};

const SERVICE_NAME: &str = "amaru";

const AMARU_LOG_VAR: &str = "AMARU_LOG";

const DEFAULT_AMARU_LOG_FILTER: &str = "amaru=info";

const AMARU_TRACE_VAR: &str = "AMARU_TRACE";

const DEFAULT_AMARU_TRACE_FILTER: &str = "amaru=debug";

type OpenTelemetryLayer<S> = Layered<OpenTelemetryFilter<S>, S>;

type OpenTelemetryFilter<S> = Filtered<
    tracing_opentelemetry::OpenTelemetryLayer<S, opentelemetry_sdk::trace::Tracer>,
    EnvFilter,
    S,
>;

type JsonLayer<S> = Layered<JsonFilter<S>, S>;

type JsonFilter<S> = Filtered<Layer<S, JsonFields, Format<Json>>, EnvFilter, S>;

#[allow(clippy::large_enum_variant)]
#[derive(Default)]
pub enum TracingSubscriber<S> {
    #[default]
    Empty,
    Registry(Registry),
    WithOpenTelemetry(OpenTelemetryLayer<S>),
    WithJson(JsonLayer<S>),
    WithBoth(JsonLayer<OpenTelemetryLayer<S>>),
}

impl TracingSubscriber<Registry> {
    pub fn new() -> Self {
        Self::Registry(tracing_subscriber::registry())
    }

    pub fn with_open_telemetry(&mut self, layer: OpenTelemetryFilter<Registry>) {
        match std::mem::take(self) {
            Self::Registry(registry) => {
                *self = TracingSubscriber::WithOpenTelemetry(registry.with(layer));
            }
            _ => panic!("'with_open_telemetry' called after 'with_json'"),
        }
    }

    pub fn with_json<F, G>(&mut self, layer_json: F, layer_both: G)
    where
        F: FnOnce() -> JsonFilter<Registry>,
        G: FnOnce() -> JsonFilter<OpenTelemetryLayer<Registry>>,
    {
        match std::mem::take(self) {
            Self::Registry(registry) => {
                *self = TracingSubscriber::WithJson(registry.with(layer_json()));
            }
            Self::WithOpenTelemetry(layered) => {
                *self = TracingSubscriber::WithBoth(layered.with(layer_both()));
            }
            _ => panic!("'with_open_telemetry' called after 'with_json'"),
        }
    }

    pub fn init(self) {
        match self {
            TracingSubscriber::Empty => unreachable!(),
            TracingSubscriber::Registry(registry) => registry.init(),
            TracingSubscriber::WithOpenTelemetry(layered) => layered.init(),
            TracingSubscriber::WithJson(layered) => layered.init(),
            TracingSubscriber::WithBoth(layered) => layered.init(),
        }
    }
}

// -----------------------------------------------------------------------------
// JSON
// -----------------------------------------------------------------------------

pub fn setup_json_traces(subscriber: &mut TracingSubscriber<Registry>) {
    let format = || tracing_subscriber::fmt::format().json();
    let events = || FmtSpan::ENTER | FmtSpan::EXIT;
    let filter = || default_filter(AMARU_TRACE_VAR, DEFAULT_AMARU_TRACE_FILTER);

    subscriber.with_json(
        || {
            tracing_subscriber::fmt::layer()
                .event_format(format())
                .fmt_fields(JsonFields::new())
                .with_span_events(events())
                .with_filter(filter())
        },
        || {
            tracing_subscriber::fmt::layer()
                .event_format(format())
                .fmt_fields(JsonFields::new())
                .with_span_events(events())
                .with_filter(filter())
        },
    )
}

// -----------------------------------------------------------------------------
// OPEN TELEMETRY
// -----------------------------------------------------------------------------

pub struct OpenTelemetryHandle {
    pub metrics: Option<SdkMeterProvider>,
    pub teardown: Box<dyn FnOnce() -> miette::Result<()>>,
}

impl Default for OpenTelemetryHandle {
    fn default() -> Self {
        OpenTelemetryHandle {
            metrics: None::<SdkMeterProvider>,
            teardown: Box::new(|| Ok(())) as Box<dyn FnOnce() -> miette::Result<()>>,
        }
    }
}

pub fn setup_open_telemetry(subscriber: &mut TracingSubscriber<Registry>) -> OpenTelemetryHandle {
    use opentelemetry::{trace::TracerProvider as _, KeyValue};
    use opentelemetry_sdk::{metrics::Temporality, Resource};

    let resource = Resource::new(vec![KeyValue::new("service.name", SERVICE_NAME)]);

    // Traces & span
    let opentelemetry_provider = opentelemetry_sdk::trace::TracerProvider::builder()
        .with_resource(resource.clone())
        .with_batch_exporter(
            opentelemetry_otlp::SpanExporter::builder()
                .with_tonic()
                .build()
                .unwrap_or_else(|e| panic!("failed to setup opentelemetry span exporter: {e}")),
            opentelemetry_sdk::runtime::Tokio,
        )
        .build();

    // Metrics
    // NOTE: We use the http exporter here because not every OTLP receivers (in particular Jaeger)
    // support gRPC for metrics.
    let metric_exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_http()
        .with_temporality(Temporality::default())
        .build()
        .unwrap_or_else(|e| panic!("unable to create metric exporter: {e:?}"));

    let metric_reader = opentelemetry_sdk::metrics::PeriodicReader::builder(
        metric_exporter,
        opentelemetry_sdk::runtime::Tokio,
    )
    .build();

    let metrics_provider = opentelemetry_sdk::metrics::SdkMeterProvider::builder()
        .with_reader(metric_reader)
        .with_resource(resource)
        .build();

    opentelemetry::global::set_meter_provider(metrics_provider.clone());

    // Subscriber
    let opentelemetry_tracer = opentelemetry_provider.tracer(SERVICE_NAME);
    let opentelemetry_layer = tracing_opentelemetry::layer()
        .with_tracer(opentelemetry_tracer)
        .with_filter(default_filter(AMARU_TRACE_VAR, DEFAULT_AMARU_TRACE_FILTER));

    subscriber.with_open_telemetry(opentelemetry_layer);

    OpenTelemetryHandle {
        metrics: Some(metrics_provider.clone()),
        teardown: Box::new(|| teardown_open_telemetry(opentelemetry_provider, metrics_provider))
            as Box<dyn FnOnce() -> miette::Result<()>>,
    }
}

fn teardown_open_telemetry(
    tracing: TracerProvider,
    metrics: SdkMeterProvider,
) -> miette::Result<()> {
    // Shut down the providers so that it flushes any remaining spans
    // TODO: we might also want to wrap this in a timeout, so we don't hold the process open forever?
    tracing.shutdown().into_diagnostic()?;
    metrics.shutdown().into_diagnostic()?;

    // This appears to be a deprecated method that will be removed soon
    // and just *releases* a reference to it, but doesn't actually call shutdown
    // still, we call it just in case until it gets removed
    // See:
    // https://github.com/tokio-rs/tracing-opentelemetry/issues/159
    // https://github.com/tokio-rs/tracing-opentelemetry/pull/175
    // https://github.com/open-telemetry/opentelemetry-rust/issues/1961
    opentelemetry::global::shutdown_tracer_provider();

    Ok(())
}

// -----------------------------------------------------------------------------
// ENV FILTER
// -----------------------------------------------------------------------------

fn default_filter(var: &str, default: &str) -> EnvFilter {
    // NOTE: We filter all logs using 'none' to avoid dependencies polluting our traces & logs,
    // which is a not so nice side-effects of the tracing library.
    EnvFilter::builder()
        .parse(format!(
            "none,{}",
            env::var(var).ok().as_deref().unwrap_or(default)
        ))
        .unwrap_or_else(|e| panic!("invalid {var} filters: {e}"))
}
