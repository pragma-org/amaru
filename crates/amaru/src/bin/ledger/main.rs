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

use amaru::{observability::setup_observability, panic::panic_handler};
use clap::{Parser, Subcommand};
use tracing::info;

mod cmd;

#[derive(Debug, Subcommand)]
enum Command {
    /// Syncs the ledger with local blocks.
    ///
    /// The ledger state must be in an already bootstrapped state (e.g. via amaru snapshots).
    Sync(cmd::sync::Args),

    #[cfg(feature = "mithril")]
    Mithril(cmd::mithril::Args),
}
#[derive(Debug, Parser)]
#[clap(name = "Amaru Ledger")]
#[clap(bin_name = "amaru-ledger")]
#[clap(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    #[clap(long, action, env("AMARU_WITH_OPEN_TELEMETRY"))]
    with_open_telemetry: bool,

    #[clap(long, action, env("AMARU_WITH_JSON_TRACES"))]
    with_json_traces: bool,

    #[clap(long, action, env("AMARU_COLOR"))]
    color: Option<bool>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    panic_handler();

    let args = Cli::parse();

    info!(
        with_open_telemetry = args.with_open_telemetry,
        with_json_traces = args.with_json_traces,
        "Started with global arguments"
    );

    let (_metrics, teardown) =
        setup_observability(args.with_open_telemetry, args.with_json_traces, args.color);

    let result = match args.command {
        Command::Sync(args) => cmd::sync::run(args).await,
        #[cfg(feature = "mithril")]
        Command::Mithril(args) => cmd::mithril::run(args).await,
    };

    // TODO: we might also want to integrate this into a graceful shutdown system, and into a panic hook
    if let Err(report) = teardown() {
        eprintln!("Failed to teardown tracing: {report}");
    }

    result
}
