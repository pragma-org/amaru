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
use observability::OpenTelemetryConfig;
use panic::panic_handler;

mod cmd;
mod metrics;
mod observability;
mod panic;

#[derive(Debug, Subcommand)]
enum Command {
    /// Bootstrap the node with needed data.
    ///
    /// This command simplifies the process of bootstrapping an Amaru
    /// node for a given network.
    ///
    /// In its current form, given a network name, a target directory
    /// and possibly a peer to connect to, it will lookup for
    /// bootstrap configuration files in `data/${network name}/`
    /// directory to download snapshots, import those snapshots into
    /// the ledger, import nonces, and import headers.
    ///
    /// **NOTE**: Only `preprod` network is supported for now.
    Bootstrap(cmd::bootstrap::Args),

    /// Run the node in all its glory.
    Daemon(cmd::daemon::Args),

    /// Import the ledger state from a CBOR export produced by a Haskell node.
    #[clap(alias = "import")]
    ImportLedgerState(cmd::import_ledger_state::Args),

    /// Import block headers from another (live) node.
    #[clap(alias = "import-chain-db")]
    ImportHeaders(cmd::import_headers::Args),

    /// Import VRF nonces intermediate states
    ImportNonces(cmd::import_nonces::Args),
}

#[derive(Debug, Parser)]
#[clap(name = "Amaru")]
#[clap(bin_name = "amaru")]
#[clap(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    #[clap(long, action, env("AMARU_WITH_OPEN_TELEMETRY"))]
    with_open_telemetry: bool,

    #[clap(long, action, env("AMARU_WITH_JSON_TRACES"))]
    with_json_traces: bool,

    #[arg(long, value_name = "STRING", env("AMARU_SERVICE_NAME"), default_value_t = DEFAULT_SERVICE_NAME.to_string())]
    service_name: String,

    #[arg(long, value_name = "URL", env("AMARU_OTLP_SPAN_URL"), default_value_t = DEFAULT_OTLP_SPAN_URL.to_string())]
    otlp_span_url: String,

    #[arg(long, value_name = "URL", env("AMARU_OTLP_METRIC_URL"), default_value_t = DEFAULT_OTLP_METRIC_URL.to_string())]
    otlp_metric_url: String,
}

const DEFAULT_SERVICE_NAME: &str = "amaru";

const DEFAULT_OTLP_SPAN_URL: &str = "http://localhost:4317";

const DEFAULT_OTLP_METRIC_URL: &str = "http://localhost:4318/v1/metrics";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    panic_handler();

    let args = Cli::parse();

    let mut subscriber = observability::TracingSubscriber::new();

    let observability::OpenTelemetryHandle { metrics, teardown } = if args.with_open_telemetry {
        observability::setup_open_telemetry(
            &OpenTelemetryConfig {
                service_name: args.service_name,
                span_url: args.otlp_span_url,
                metric_url: args.otlp_metric_url,
            },
            &mut subscriber,
        )
    } else {
        observability::OpenTelemetryHandle::default()
    };

    if args.with_json_traces {
        observability::setup_json_traces(&mut subscriber);
    }

    subscriber.init();
    let result = match args.command {
        Command::Daemon(args) => cmd::daemon::run(args, metrics).await,
        Command::ImportLedgerState(args) => cmd::import_ledger_state::run(args).await,
        Command::ImportHeaders(args) => cmd::import_headers::run(args).await,
        Command::ImportNonces(args) => cmd::import_nonces::run(args).await,
        Command::Bootstrap(args) => cmd::bootstrap::run(args).await,
    };

    // TODO: we might also want to integrate this into a graceful shutdown system, and into a panic hook
    if let Err(report) = teardown() {
        eprintln!("Failed to teardown tracing: {report}");
    }

    result
}
