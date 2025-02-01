// Copyright 2024 PRAGMA
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

use clap::{Parser, Subcommand};
use miette::IntoDiagnostic;
use opentelemetry_sdk::{metrics::SdkMeterProvider, trace::TracerProvider};
use panic::panic_handler;
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

mod cmd;
mod config;
mod exit;
mod metrics;
mod panic;

pub const SERVICE_NAME: &str = "amaru";

pub const AMARU_LOG_VAR: &str = "AMARU_LOG";

pub const DEFAULT_AMARU_LOG_FILTER: &str = "amaru=info";

pub const AMARU_TRACE_VAR: &str = "AMARU_TRACE";

pub const DEFAULT_AMARU_TRACE_FILTER: &str = "amaru=debug";

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the node in all its glory.
    Daemon(cmd::daemon::Args),

    /// Import the ledger state from a CBOR export produced by the Haskell node.
    Import(cmd::import::Args),

    /// Import the chain DB from another node
    ImportChainDB(cmd::import_chain_db::Args),
}

#[derive(Debug, Parser)]
#[clap(name = "Amaru")]
#[clap(bin_name = "amaru")]
#[clap(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
    #[clap(long, action)]
    with_open_telemetry: bool,
    #[clap(long, action)]
    with_json_traces: bool,
}

#[allow(clippy::large_enum_variant)]
#[derive(Default)]
enum TracingSubscriber<S> {
    #[default]
    Empty,
    Registry(Registry),
    WithOpenTelemetry(OpenTelemetryLayer<S>),
    WithJson(JsonLayer<S>),
    WithBoth(JsonLayer<OpenTelemetryLayer<S>>),
}

type OpenTelemetryLayer<S> = Layered<OpenTelemetryFilter<S>, S>;

type OpenTelemetryFilter<S> = Filtered<
    tracing_opentelemetry::OpenTelemetryLayer<S, opentelemetry_sdk::trace::Tracer>,
    EnvFilter,
    S,
>;

type JsonLayer<S> = Layered<JsonFilter<S>, S>;

type JsonFilter<S> = Filtered<Layer<S, JsonFields, Format<Json>>, EnvFilter, S>;

impl TracingSubscriber<Registry> {
    fn new() -> Self {
        Self::Registry(tracing_subscriber::registry())
    }

    fn with_open_telemetry(&mut self, layer: OpenTelemetryFilter<Registry>) {
        match std::mem::take(self) {
            Self::Registry(registry) => {
                *self = TracingSubscriber::WithOpenTelemetry(registry.with(layer));
            }
            _ => panic!("'with_open_telemetry' called after 'with_json'"),
        }
    }

    fn with_json<F, G>(&mut self, layer_json: F, layer_both: G)
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

    fn init(self) {
        match self {
            TracingSubscriber::Empty => unreachable!(),
            TracingSubscriber::Registry(registry) => registry.init(),
            TracingSubscriber::WithOpenTelemetry(layered) => layered.init(),
            TracingSubscriber::WithJson(layered) => layered.init(),
            TracingSubscriber::WithBoth(layered) => layered.init(),
        }
    }
}

#[tokio::main]
async fn main() -> miette::Result<()> {
    panic_handler();

    let args = Cli::parse();

    let mut subscriber = TracingSubscriber::new();

    let (metrics, teardown) = if args.with_open_telemetry {
        let (otl, metrics) = setup_open_telemetry(&mut subscriber);
        (
            Some(metrics.clone()),
            Box::new(|| teardown_open_telemetry(otl, metrics))
                as Box<dyn FnOnce() -> miette::Result<()>>,
        )
    } else {
        (
            None::<SdkMeterProvider>,
            Box::new(|| Ok(())) as Box<dyn FnOnce() -> miette::Result<()>>,
        )
    };

    if args.with_json_traces {
        setup_json_traces(&mut subscriber);
    }

    subscriber.init();

    let result = match args.command {
        Command::Daemon(args) => cmd::daemon::run(args, metrics).await,
        Command::Import(args) => cmd::import::run(args).await,
        Command::ImportChainDB(args) => cmd::import_chain_db::run(args).await,
    };

    // TODO: we might also want to integrate this into a graceful shutdown system, and into a panic hook
    if let Err(report) = teardown() {
        eprintln!("Failed to teardown tracing: {report}");
    }

    result
}

fn setup_json_traces(subscriber: &mut TracingSubscriber<Registry>) {
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

fn setup_open_telemetry(
    subscriber: &mut TracingSubscriber<Registry>,
) -> (TracerProvider, SdkMeterProvider) {
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

    (opentelemetry_provider, metrics_provider)
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
