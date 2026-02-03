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

use amaru::{
    observability::{Color, setup_observability},
    panic::panic_handler,
};

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use std::sync::LazyLock;
use tracing::info;

mod cmd;
mod pid;

mod built_info {
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

/// Lazily initialized version string including git commit SHA.
/// This provides a &'static str without manually leaking memory.
static VERSION: LazyLock<String> = LazyLock::new(|| {
    let version = built_info::PKG_VERSION;
    match (built_info::GIT_COMMIT_HASH_SHORT, built_info::GIT_DIRTY) {
        (Some(sha), Some(true)) => format!("{version} ({sha}+dirty)"),
        (Some(sha), _) => format!("{version} ({sha})"),
        _ => version.to_string(),
    }
});

#[derive(Debug, Subcommand)]
enum Command {
    /// Bootstrap the node with needed data.
    ///
    /// This command simplifies the process of bootstrapping an Amaru node for any given well-known network:
    ///
    ///   - mainnet
    ///   - preprod
    ///   - preview
    ///
    /// It is a combination of the following fine-grained steps:
    ///
    ///   - import-ledger-state
    ///   - import-headers
    ///   - import-nonces
    #[clap(verbatim_doc_comment)]
    Bootstrap(cmd::bootstrap::Args),

    /// Convert ledger state as produced by Haskell node into data suitable
    /// for amaru.
    ///
    /// This command allows one to take a snapshot as produced by the
    /// `LedgerDB` in the Haskell node and extract the informations
    /// relevant to bootstrap an Amaru node, namely:
    ///
    /// * The `NewEpochState` part of the snapshot that contains the
    ///   core state of the ledger, which is written to a file whose
    ///   name is the point at which the snapshot was produced,
    /// * The`Nonces` from the `HeaderState` which are written to a `nonces.json` file.
    ConvertLedgerState(cmd::convert_ledger_state::Args),

    /// Dump the content of the chain database for troubleshooting purposes.
    ///
    /// This command dumps the _whole_ content of the chain database in a human-readable format:
    ///  - Headers (hash + hex-encoded body)
    ///  - Parent-child relationships between headers
    ///  - Nonces
    ///  - Blocks
    ///  - Best chain anchor, tip and length
    ///
    DumpChainDB(cmd::dump_chain_db::Args),

    /// Dump all registered trace schemas as JSON Schema.
    ///
    /// This command outputs all registered trace schemas in JSON Schema format.
    /// Useful for documentation, tooling, and validation.
    #[command(name = "dump-traces-schema")]
    DumpTracesSchema(cmd::dump_schemas::Args),

    /// Fetch specified headers
    FetchChainHeaders(cmd::fetch_chain_headers::Args),

    /// Import block headers
    #[clap(alias = "import-chain-db")]
    ImportHeaders(cmd::import_headers::Args),

    /// Import the ledger state from a CBOR export produced by a Haskell node.
    #[clap(alias = "import")]
    ImportLedgerState(cmd::import_ledger_state::Args),

    /// Import VRF nonces intermediate states
    ImportNonces(cmd::import_nonces::Args),

    /// Migrate the chain database to the current version.
    ///
    /// This command is only relevant when one upgrades Amaru to a newer version that
    /// requires changes in the database format.
    MigrateChainDB(cmd::migrate_chain_db::Args),

    /// Reset the ledger database to the beginning of a specific epoch
    ResetToEpoch(cmd::reset_to_epoch::Args),

    /// Run the node in all its glory.
    #[command(alias = "daemon")]
    Run(cmd::run::Args),
}

#[derive(Debug, Parser)]
#[clap(name = "Amaru")]
#[clap(bin_name = "amaru")]
#[clap(author, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    #[clap(long, action, env("AMARU_WITH_OPEN_TELEMETRY"))]
    with_open_telemetry: bool,

    #[clap(long, action, env("AMARU_WITH_JSON_TRACES"))]
    with_json_traces: bool,

    #[clap(long, action, env("AMARU_COLOR"))]
    color: Option<Color>,
}

// TODO(rkuhn): properly measure and design the Tokio runtime setup we need.
// (probably one runtime for network with 1-2 threads, one for CPU-bound tasks according to parallelism,
// one for running the consensus pipeline incl. Store access with 2+ threads)
#[expect(clippy::unwrap_used)]
#[tokio::main(worker_threads = 4)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    panic_handler();

    let matches = <Cli as CommandFactory>::command()
        .version(VERSION.as_str())
        .get_matches();
    let args = <Cli as FromArgMatches>::from_arg_matches(&matches)?;

    // Skip observability setup for dump-traces-schema to avoid polluting stderr
    let skip_logging = matches!(args.command, Command::DumpTracesSchema(_));

    let (metrics, teardown) = if skip_logging {
        (
            None,
            Box::new(|| Ok(())) as Box<dyn FnOnce() -> Result<(), Box<dyn std::error::Error>>>,
        )
    } else {
        let (m, t) = setup_observability(
            args.with_open_telemetry,
            args.with_json_traces,
            Color::is_enabled(args.color),
        );
        (Some(m), t)
    };

    if !skip_logging {
        info!(
            with_open_telemetry = args.with_open_telemetry,
            with_json_traces = args.with_json_traces,
            "Started with global arguments"
        );
    }

    let result = match args.command {
        Command::Run(args) => cmd::run::run(args, metrics.unwrap()).await,
        Command::ImportLedgerState(args) => cmd::import_ledger_state::run(args).await,
        Command::ImportHeaders(args) => cmd::import_headers::run(args).await,
        Command::ImportNonces(args) => cmd::import_nonces::run(args).await,
        Command::Bootstrap(args) => cmd::bootstrap::run(args).await,
        Command::FetchChainHeaders(args) => cmd::fetch_chain_headers::run(args).await,
        Command::ConvertLedgerState(args) => cmd::convert_ledger_state::run(args).await,
        Command::DumpChainDB(args) => cmd::dump_chain_db::run(args).await,
        Command::DumpTracesSchema(args) => cmd::dump_schemas::run(args).await,
        Command::MigrateChainDB(args) => cmd::migrate_chain_db::run(args).await,
        Command::ResetToEpoch(args) => cmd::reset_to_epoch::run(args).await,
    };

    // TODO: we might also want to integrate this into a graceful shutdown system, and into a panic hook
    if let Err(report) = teardown() {
        eprintln!("Failed to teardown tracing: {report}");
    }

    result
}
