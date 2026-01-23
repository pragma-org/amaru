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

use opentelemetry::trace::TracerProvider;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::{metrics::SdkMeterProvider, trace::SdkTracerProvider};
use opentelemetry_semantic_conventions::resource::SERVICE_NAME;
use std::{
    env::VarError,
    error::Error,
    io::{self, IsTerminal},
    str::FromStr,
};
use tracing::{info, warn};
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

const AMARU_LOG_VAR: &str = "AMARU_LOG";

const DEFAULT_AMARU_LOG_FILTER: &str = "error,amaru=debug";

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

type DelayedWarning = Option<Box<dyn FnOnce()>>;

#[expect(clippy::large_enum_variant)]
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

    #[expect(clippy::panic)]
    #[expect(clippy::wildcard_enum_match_arm)]
    pub fn with_open_telemetry(&mut self, layer: OpenTelemetryFilter<Registry>) {
        match std::mem::take(self) {
            Self::Registry(registry) => {
                *self = TracingSubscriber::WithOpenTelemetry(registry.with(layer));
            }
            _ => panic!("'with_open_telemetry' called after 'with_json'"),
        }
    }

    #[expect(clippy::panic)]
    #[expect(clippy::wildcard_enum_match_arm)]
    pub fn with_json<F, G>(&mut self, layer_json: F, layer_both: G) -> DelayedWarning
    where
        F: FnOnce() -> (JsonFilter<Registry>, DelayedWarning),
        G: FnOnce() -> (JsonFilter<OpenTelemetryLayer<Registry>>, DelayedWarning),
    {
        match std::mem::take(self) {
            Self::Registry(registry) => {
                let (layer, warning) = layer_json();
                *self = TracingSubscriber::WithJson(registry.with(layer));
                warning
            }
            Self::WithOpenTelemetry(layered) => {
                let (layer, warning) = layer_both();
                *self = TracingSubscriber::WithBoth(layered.with(layer));
                warning
            }
            _ => panic!("'with_open_telemetry' called after 'with_json'"),
        }
    }

    pub fn init(self, color: bool) {
        let (default_filter, warning) = new_default_filter(AMARU_LOG_VAR, DEFAULT_AMARU_LOG_FILTER);

        let log_format = || tracing_subscriber::fmt::format().with_ansi(color).compact();
        let log_writer = || io::stderr as fn() -> io::Stderr;
        let log_events = || FmtSpan::CLOSE;
        let log_filter = || default_filter;

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
            TracingSubscriber::WithJson(layered) => layered.init(),
            TracingSubscriber::WithBoth(layered) => layered.init(),
        };

        if let Some(notify) = warning {
            notify();
        }
    }
}

// -----------------------------------------------------------------------------
// JSON TRACES
// -----------------------------------------------------------------------------

pub fn setup_json_traces(subscriber: &mut TracingSubscriber<Registry>) -> DelayedWarning {
    let format = || {
        tracing_subscriber::fmt::format()
            .json()
            .with_span_list(false)
    };
    let events = || FmtSpan::ENTER | FmtSpan::EXIT;
    let filter = || new_default_filter(AMARU_TRACE_VAR, DEFAULT_AMARU_TRACE_FILTER);

    subscriber.with_json(
        || {
            let (default_filter, warning) = filter();
            (
                tracing_subscriber::fmt::layer()
                    .event_format(format())
                    .fmt_fields(JsonFields::new())
                    .with_span_events(events())
                    .with_filter(default_filter),
                warning,
            )
        },
        || {
            let (default_filter, warning) = filter();
            (
                tracing_subscriber::fmt::layer()
                    .event_format(format())
                    .fmt_fields(JsonFields::new())
                    .with_span_events(events())
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

pub const DEFAULT_OTLP_SERVICE_NAME: &str = "amaru";

pub const DEFAULT_OTLP_METRIC_URL: &str = "http://localhost:4318/v1/metrics";

#[expect(clippy::panic)]
pub fn setup_open_telemetry(
    subscriber: &mut TracingSubscriber<Registry>,
) -> (OpenTelemetryHandle, DelayedWarning) {
    use opentelemetry::KeyValue;
    use opentelemetry_sdk::{Resource, metrics::Temporality};

    let resource = Resource::builder()
        .with_attribute(KeyValue::new(SERVICE_NAME, DEFAULT_OTLP_SERVICE_NAME))
        .build();

    // Traces & span
    let opentelemetry_provider = SdkTracerProvider::builder()
        .with_resource(resource.clone())
        .with_batch_exporter(
            opentelemetry_otlp::SpanExporter::builder()
                .with_tonic()
                .build()
                .unwrap_or_else(|e| panic!("failed to setup opentelemetry span exporter: {e}")),
        )
        .build();

    // Metrics
    // NOTE: We use the http exporter here because not every OTLP receivers (in particular Jaeger)
    // support gRPC for metrics.
    let metric_exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_http()
        .with_endpoint(DEFAULT_OTLP_METRIC_URL)
        .with_temporality(Temporality::default())
        .build()
        .unwrap_or_else(|e| panic!("unable to create metric exporter: {e:?}"));

    let metric_reader =
        opentelemetry_sdk::metrics::PeriodicReader::builder(metric_exporter).build();

    let metrics_provider = opentelemetry_sdk::metrics::SdkMeterProvider::builder()
        .with_reader(metric_reader)
        .with_resource(resource)
        .build();

    // FIXME RK: this doesn’t seem to be used anywhere?
    opentelemetry::global::set_meter_provider(metrics_provider.clone());

    // Subscriber
    let opentelemetry_tracer = opentelemetry_provider.tracer(DEFAULT_OTLP_SERVICE_NAME);
    let (default_filter, warning) = new_default_filter(AMARU_TRACE_VAR, DEFAULT_AMARU_TRACE_FILTER);

    let opentelemetry_layer = tracing_opentelemetry::layer()
        .with_tracer(opentelemetry_tracer)
        .with_level(true)
        .with_filter(default_filter);

    subscriber.with_open_telemetry(opentelemetry_layer);

    (
        OpenTelemetryHandle {
            metrics: Some(metrics_provider.clone()),
            teardown: Box::new(|| {
                teardown_open_telemetry(opentelemetry_provider, metrics_provider)
            }),
        },
        warning,
    )
}

fn teardown_open_telemetry(
    tracing: SdkTracerProvider,
    metrics: SdkMeterProvider,
) -> Result<(), Box<dyn std::error::Error>> {
    // Shut down the providers so that it flushes any remaining spans
    // TODO: we might also want to wrap this in a timeout, so we don't hold the process open forever?
    tracing.shutdown()?;
    metrics.shutdown()?;

    Ok(())
}

// -----------------------------------------------------------------------------
// ENV FILTER
// -----------------------------------------------------------------------------

fn new_default_filter(var: &str, default: &str) -> (EnvFilter, DelayedWarning) {
    match EnvFilter::try_from_env(var) {
        Ok(filter) => (filter, None),
        Err(e) => {
            // Notice stashed for when the tracing system is up.
            let fallback = default.to_string();
            let var = var.to_string();
            let warning = match e.source().and_then(|e| e.downcast_ref::<VarError>()) {
                Some(VarError::NotPresent) => {
                    Box::new(move || info!(var, fallback, "unspecified ENV variable"))
                        as Box<dyn FnOnce()>
                }
                _ => Box::new(move || warn!(var, fallback, reason = %e, "invalid ENV variable"))
                    as Box<dyn FnOnce()>,
            };

            #[expect(clippy::expect_used)]
            let filter = EnvFilter::try_new(default).expect("invalid default filter");
            (filter, Some(warning))
        }
    }
}

pub fn setup_observability(
    with_open_telemetry: bool,
    with_json_traces: bool,
    color: bool,
) -> (
    Option<SdkMeterProvider>,
    Box<dyn FnOnce() -> Result<(), Box<dyn std::error::Error>>>,
) {
    let mut subscriber = TracingSubscriber::new();

    let (OpenTelemetryHandle { metrics, teardown }, warning_otlp) = if with_open_telemetry {
        setup_open_telemetry(&mut subscriber)
    } else {
        (OpenTelemetryHandle::default(), None)
    };

    let warning_json = if with_json_traces {
        setup_json_traces(&mut subscriber)
    } else {
        None
    };

    subscriber.init(color);

    // NOTE: Both warnings are bound to the same ENV var, so `.or` prevents from logging it twice.
    if let Some(notify) = warning_otlp.or(warning_json) {
        notify();
    }

    (metrics, teardown)
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
            "never" => Ok(Color::Never),
            "always" => Ok(Color::Always),
            "auto" => Ok(Color::Auto),
            _ => Err("valid color settings are 'never', 'always' or 'auto'"),
        }
    }
}
impl Color {
    pub fn is_enabled(this: Option<Self>) -> bool {
        match this {
            Some(Color::Never) => false,
            Some(Color::Always) => true,
            Some(Color::Auto) => std::io::stderr().is_terminal(),
            None => {
                if std::env::var("NO_COLOR").iter().any(|s| !s.is_empty()) {
                    false
                } else {
                    std::io::stderr().is_terminal()
                }
            }
        }
    }
}
