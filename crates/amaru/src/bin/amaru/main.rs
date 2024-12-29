use clap::{Parser, Subcommand};
use opentelemetry::metrics::Counter;
use panic::panic_handler;
use std::env;

mod cmd;
mod config;
mod exit;
mod panic;

pub const SERVICE_NAME: &str = "amaru";

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the node in all its glory.
    Daemon(cmd::daemon::Args),

    /// Import the ledger state from a CBOR export produced by the Haskell node.
    Import(cmd::import::Args),
}

#[derive(Debug, Parser)]
#[clap(name = "Amaru")]
#[clap(bin_name = "amaru")]
#[clap(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[tokio::main]
async fn main() -> miette::Result<()> {
    panic_handler();

    let counter = setup_tracing();

    let args = Cli::parse();

    match args.command {
        Command::Daemon(args) => cmd::daemon::run(args, counter).await,
        Command::Import(args) => cmd::import::run(args).await,
    }
}

pub fn setup_tracing() -> Counter<u64> {
    use opentelemetry::{metrics::MeterProvider, trace::TracerProvider as _, KeyValue};
    use opentelemetry_sdk::{metrics::Temporality, Resource};
    use tracing_subscriber::{prelude::*, *};

    const AMARU_LOG: &str = "AMARU_LOG";
    const AMARU_DEV_LOG: &str = "AMARU_DEV_LOG";

    // Enabling filtering from env var; but disable gasket low-level traces regardless. We use the
    // env directive filtering here instead of the .or() / .and() Rust API provided on EnvFilter so
    // that we allow users to override those settings should they ever want to.
    let filter = |env: &str| {
        EnvFilter::builder()
            .parse(format!(
                "info,gasket=error,{}",
                if env == AMARU_DEV_LOG {
                    env::var(env)
                        .or_else(|_| env::var(AMARU_LOG))
                        .ok()
                        .unwrap_or_default()
                } else {
                    env::var(env).ok().unwrap_or_default()
                }
            ))
            .unwrap_or_else(|e| panic!("invalid log/trace filters: {e}"))
    };

    let resource = Resource::new(vec![KeyValue::new("service.name", SERVICE_NAME)]);

    // Traces & span
    let opentelemetry_layer = tracing_opentelemetry::layer()
        .with_tracer(
            opentelemetry_sdk::trace::TracerProvider::builder()
                .with_resource(resource.clone())
                .with_batch_exporter(
                    opentelemetry_otlp::SpanExporter::builder()
                        .with_tonic()
                        .build()
                        .unwrap_or_else(|e| {
                            panic!("failed to setup opentelemetry span exporter: {e}")
                        }),
                    opentelemetry_sdk::runtime::Tokio,
                )
                .build()
                .tracer(SERVICE_NAME),
        )
        .with_filter(filter(AMARU_LOG));

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

    let provider = opentelemetry_sdk::metrics::SdkMeterProvider::builder()
        .with_reader(metric_reader)
        .with_resource(resource)
        .build();

    let meter = provider.meter("amaru");

    opentelemetry::global::set_meter_provider(provider);

    // Subscriber
    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .event_format(fmt::format().with_ansi(true).pretty())
                .with_span_events(fmt::format::FmtSpan::ENTER | fmt::format::FmtSpan::EXIT)
                .with_filter(filter(AMARU_DEV_LOG)),
        )
        .with(opentelemetry_layer)
        .init();

    let counter = meter.u64_counter("block.count").build();

    counter
}
