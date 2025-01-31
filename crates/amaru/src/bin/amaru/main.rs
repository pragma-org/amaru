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
use opentelemetry::metrics::Counter;
use opentelemetry_sdk::{metrics::SdkMeterProvider, trace::TracerProvider};
use panic::panic_handler;
use std::env;

mod cmd;
mod config;
mod exit;
mod metrics;
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
    #[clap(long, short, action)]
    disable_telemetry_export: bool,
}

#[tokio::main]
async fn main() -> miette::Result<()> {
    panic_handler();

    let args = Cli::parse();

    let (tracing, metrics, counter) = setup_tracing(args.disable_telemetry_export);

    let result = match args.command {
        Command::Daemon(args) => cmd::daemon::run(args, counter, metrics.clone()).await,
        Command::Import(args) => cmd::import::run(args).await,
    };

    // TODO: we might also want to integrate this into a graceful shutdown system, and into a panic hook
    if let Err(report) = teardown_tracing(tracing, metrics) {
        eprintln!("Failed to teardown tracing: {report}");
    }

    result
}

pub fn setup_tracing(
    disable_telemetry_export: bool,
) -> (TracerProvider, SdkMeterProvider, Counter<u64>) {
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

    let meter = metrics_provider.meter("amaru");

    opentelemetry::global::set_meter_provider(metrics_provider.clone());

    let fmt_layer = fmt::layer()
        .event_format(fmt::format().with_ansi(true).pretty())
        .with_span_events(fmt::format::FmtSpan::ENTER | fmt::format::FmtSpan::EXIT)
        .with_filter(filter(AMARU_DEV_LOG));

    // Subscriber

    if disable_telemetry_export {
        tracing_subscriber::registry().with(fmt_layer).init();
    } else {
        let opentelemetry_tracer = opentelemetry_provider.tracer(SERVICE_NAME);
        let opentelemetry_layer = tracing_opentelemetry::layer()
            .with_tracer(opentelemetry_tracer)
            .with_filter(filter(AMARU_LOG));

        tracing_subscriber::registry()
            .with(fmt_layer)
            .with(opentelemetry_layer)
            .init();
    }

    let counter = meter.u64_counter("block.count").build();

    (opentelemetry_provider, metrics_provider, counter)
}

pub fn teardown_tracing(tracing: TracerProvider, metrics: SdkMeterProvider) -> miette::Result<()> {
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
