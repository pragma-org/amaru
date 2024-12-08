use clap::{Parser, Subcommand};
use std::env;

mod cmd;
mod config;
mod exit;

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
    setup_tracing();

    let args = Cli::parse();

    match args.command {
        Command::Daemon(args) => cmd::daemon::run(args).await,
        Command::Import(args) => cmd::import::run(args).await,
    }
}

pub fn setup_tracing() {
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

    let pretty = || fmt::format().with_ansi(true).pretty();

    let fmt_span = || fmt::format::FmtSpan::ENTER | fmt::format::FmtSpan::EXIT;

    // Layer for open-telemetry. Requires an opentelemetry-compatible collector to run
    // as an additional service.
    #[cfg(feature = "telemetry")]
    {
        use opentelemetry::{trace::TracerProvider as _, KeyValue};
        use opentelemetry_sdk::Resource;

        let opentelemetry_layer = || {
            tracing_opentelemetry::layer()
                .with_tracer(
                    opentelemetry_sdk::trace::TracerProvider::builder()
                        .with_resource(Resource::new(vec![KeyValue::new(
                            "service.name",
                            SERVICE_NAME,
                        )]))
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
                .with_filter(filter(AMARU_LOG))
        };

        opentelemetry::global::set_text_map_propagator(
            opentelemetry_sdk::propagation::TraceContextPropagator::new(),
        );

        tracing_subscriber::registry()
            .with(
                fmt::layer()
                    .event_format(pretty())
                    .with_span_events(fmt_span())
                    .with_filter(filter(AMARU_DEV_LOG)),
            )
            .with(opentelemetry_layer())
            .init();
    }

    #[cfg(not(feature = "telemetry"))]
    {
        tracing_subscriber::registry()
            .with(
                fmt::layer()
                    .event_format(pretty())
                    .with_span_events(fmt_span())
                    .with_filter(filter(AMARU_DEV_LOG)),
            )
            .init();
    }
}
