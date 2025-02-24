use opentelemetry_sdk::{metrics::SdkMeterProvider, trace::TracerProvider};
use std::{
    env,
    io::{self},
};
use tracing_subscriber::{
    EnvFilter, Registry,
    filter::Filtered,
    fmt::{
        Layer,
        format::{FmtSpan, Format, Json, JsonFields},
    },
    layer::{Layered, SubscriberExt},
    prelude::*,
    util::SubscriberInitExt,
};

const SERVICE_NAME: &str = "amaru";

const AMARU_LOG_VAR: &str = "AMARU_LOG";

const DEFAULT_AMARU_LOG_FILTER: &str = "amaru=debug";

const AMARU_TRACE_VAR: &str = "AMARU_TRACE";

const DEFAULT_AMARU_TRACE_FILTER: &str = "amaru=trace";

// -----------------------------------------------------------------------------
// TracingSubscriber
// -----------------------------------------------------------------------------

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

    #[allow(clippy::panic)]
    pub fn with_open_telemetry(&mut self, layer: OpenTelemetryFilter<Registry>) {
        match std::mem::take(self) {
            Self::Registry(registry) => {
                *self = TracingSubscriber::WithOpenTelemetry(registry.with(layer));
            }
            _ => panic!("'with_open_telemetry' called after 'with_json'"),
        }
    }

    #[allow(clippy::panic)]
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
        let log_format = || tracing_subscriber::fmt::format().with_ansi(true).compact();
        let log_writer = || io::stderr as fn() -> io::Stderr;
        let log_events = || FmtSpan::ACTIVE;
        let log_filter = || default_filter(AMARU_LOG_VAR, DEFAULT_AMARU_LOG_FILTER);

        match self {
            TracingSubscriber::Empty => unreachable!(),
            TracingSubscriber::Registry(registry) => registry
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_writer(log_writer())
                        .event_format(log_format())
                        .with_span_events(log_events())
                        .with_filter(log_filter()),
                )
                .init(),
            TracingSubscriber::WithOpenTelemetry(layered) => layered
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_writer(log_writer())
                        .event_format(log_format())
                        .with_span_events(log_events())
                        .with_filter(log_filter()),
                )
                .init(),
            TracingSubscriber::WithJson(layered) => layered
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_writer(log_writer())
                        .event_format(log_format())
                        .with_span_events(log_events())
                        .with_filter(log_filter()),
                )
                .init(),
            TracingSubscriber::WithBoth(layered) => layered
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_writer(log_writer())
                        .event_format(log_format())
                        .with_span_events(log_events())
                        .with_filter(log_filter()),
                )
                .init(),
        }
    }
}

// -----------------------------------------------------------------------------
// JSON TRACES
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
    pub teardown: Box<dyn FnOnce() -> Result<(), Box<dyn std::error::Error>>>,
}

impl Default for OpenTelemetryHandle {
    fn default() -> Self {
        OpenTelemetryHandle {
            metrics: None::<SdkMeterProvider>,
            teardown: Box::new(|| Ok(()))
                as Box<dyn FnOnce() -> Result<(), Box<dyn std::error::Error>>>,
        }
    }
}

#[allow(clippy::panic)]
pub fn setup_open_telemetry(subscriber: &mut TracingSubscriber<Registry>) -> OpenTelemetryHandle {
    use opentelemetry::{KeyValue, trace::TracerProvider as _};
    use opentelemetry_sdk::{Resource, metrics::Temporality};

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
        teardown: Box::new(|| teardown_open_telemetry(opentelemetry_provider, metrics_provider)),
    }
}

fn teardown_open_telemetry(
    tracing: TracerProvider,
    metrics: SdkMeterProvider,
) -> Result<(), Box<dyn std::error::Error>> {
    // Shut down the providers so that it flushes any remaining spans
    // TODO: we might also want to wrap this in a timeout, so we don't hold the process open forever?
    tracing.shutdown()?;
    metrics.shutdown()?;

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
#[allow(clippy::panic)]
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
