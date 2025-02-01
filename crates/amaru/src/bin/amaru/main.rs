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
use tracing_subscriber::EnvFilter;

mod cmd;
mod config;
mod exit;
mod metrics;
mod panic;

pub const SERVICE_NAME: &str = "amaru";

pub const AMARU_LOG: &str = "AMARU_LOG";

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
    #[clap(long, short, action)]
    disable_telemetry_export: bool,
}

#[tokio::main]
async fn main() -> miette::Result<()> {
    panic_handler();

    let args = Cli::parse();

    let (metrics, teardown) = if !args.disable_telemetry_export {
        let (otl, metrics) = setup_open_telemetry();
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

pub fn setup_open_telemetry() -> (TracerProvider, SdkMeterProvider) {
    use opentelemetry::{trace::TracerProvider as _, KeyValue};
    use opentelemetry_sdk::{metrics::Temporality, Resource};
    use tracing_subscriber::prelude::*;

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
        .with_filter(default_trace_filter());

    tracing_subscriber::registry()
        .with(opentelemetry_layer)
        .init();

    (opentelemetry_provider, metrics_provider)
}

pub fn teardown_open_telemetry(
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

fn default_trace_filter() -> EnvFilter {
    // NOTE: We filter all logs using 'none' to avoid dependencies polluting our traces & logs,
    // which is a not so nice side-effects of the tracing library.
    EnvFilter::builder()
        .parse(format!(
            "none,{}",
            env::var(AMARU_LOG).ok().as_deref().unwrap_or("amaru=debug")
        ))
        .unwrap_or_else(|e| panic!("invalid log/trace filters: {e}"))
}

fn default_log_filter() -> EnvFilter {
    EnvFilter::builder().parse("none,amaru=info").unwrap()
}
