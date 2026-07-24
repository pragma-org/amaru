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
    version,
};
use cli::Command;

mod cli;
mod cmd;
mod pid;

// TODO(rkuhn): properly measure and design the Tokio runtime setup we need.
// (probably one runtime for network with 1-2 threads, one for CPU-bound tasks according to parallelism,
// one for running the consensus pipeline incl. Store access with 2+ threads)
#[expect(clippy::unwrap_used)]
#[tokio::main(worker_threads = 4)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    panic_handler();

    let cli = cli::parse(version::display_version())?;
    if cli.command.show_alternative_help()? {
        return Ok(());
    }

    let color_enabled = Color::is_enabled(cli.color);

    let (metrics, teardown) = if cli.command.skip_logging() {
        (None, Box::new(|| Ok(())) as Box<dyn FnOnce() -> Result<(), Box<dyn std::error::Error>>>)
    } else {
        let (m, t) = setup_observability(cli.with_open_telemetry, cli.with_json_traces, color_enabled, &cli.command);
        (Some(m), t)
    };

    let result = match cli.command {
        Command::Node(node_cmd) => match node_cmd {
            cmd::node::NodeCommand::Run(args) => cmd::node::run::run(args, metrics.unwrap_or(None)).await,
            cmd::node::NodeCommand::Bootstrap(args) => cmd::node::bootstrap::run(args).await,
        },
        Command::Snapshot(snap_cmd) => match snap_cmd {
            cmd::snapshot::SnapshotCommand::Create(args) => cmd::snapshot::create::run(args).await,
            cmd::snapshot::SnapshotCommand::Publish(args) => cmd::snapshot::publish::run(args).await,
        },
        Command::Dev(dev_cmd) => match dev_cmd {
            cmd::dev::DevCommand::Chain(chain_cmd) => match chain_cmd {
                cmd::dev::chain::ChainCommand::Ancestors(args) => cmd::dev::chain::ancestors::run(args).await,
                cmd::dev::chain::ChainCommand::BestChain(args) => cmd::dev::chain::best_chain::run(args).await,
                cmd::dev::chain::ChainCommand::Children(args) => cmd::dev::chain::children::run(args).await,
                cmd::dev::chain::ChainCommand::ClearInvalid(args) => cmd::dev::chain::clear_invalid::run(args).await,
                cmd::dev::chain::ChainCommand::Dump(args) => cmd::dev::chain::dump::run(args).await,
                cmd::dev::chain::ChainCommand::Fetch(args) => cmd::dev::chain::fetch::run(args).await,
                cmd::dev::chain::ChainCommand::Migrate(args) => cmd::dev::chain::migrate::run(args).await,
                cmd::dev::chain::ChainCommand::Prune(args) => cmd::dev::chain::prune::run(args).await,
                cmd::dev::chain::ChainCommand::Remove(args) => cmd::dev::chain::remove::run(args).await,
            },
            cmd::dev::DevCommand::Ledger(ledger_cmd) => match ledger_cmd {
                cmd::dev::ledger::LedgerCommand::Reset(args) => cmd::dev::ledger::reset::run(args).await,
                cmd::dev::ledger::LedgerCommand::Convert(args) => cmd::dev::ledger::convert::run(args).await,
                #[cfg(feature = "mithril")]
                cmd::dev::ledger::LedgerCommand::Mithril(args) => cmd::dev::ledger::mithril::run(args).await,
                cmd::dev::ledger::LedgerCommand::Nonces(nonces_cmd) => match nonces_cmd {
                    cmd::dev::ledger::nonces::NoncesCommand::Get(args) => {
                        cmd::dev::ledger::nonces::get::run(args).await
                    }
                    cmd::dev::ledger::nonces::NoncesCommand::Set(args) => {
                        cmd::dev::ledger::nonces::set::run(args).await
                    }
                },
                cmd::dev::ledger::LedgerCommand::States(states_cmd) => match states_cmd {
                    cmd::dev::ledger::states::StatesCommand::List(args) => {
                        cmd::dev::ledger::states::list::run(args).await
                    }
                    cmd::dev::ledger::states::StatesCommand::Import(args) => {
                        cmd::dev::ledger::states::import::run(args).await
                    }
                    cmd::dev::ledger::states::StatesCommand::Remove(args) => {
                        cmd::dev::ledger::states::remove::run(args).await
                    }
                },
                cmd::dev::ledger::LedgerCommand::Sync(args) => cmd::dev::ledger::sync::run(args).await,
            },
            cmd::dev::DevCommand::Traces(traces_cmd) => match traces_cmd {
                cmd::dev::traces::TracesCommand::Dump(args) => cmd::dev::traces::dump::run(args).await,
                cmd::dev::traces::TracesCommand::Schema(args) => cmd::dev::traces::dump::run(args).await,
            },
        },
        Command::ShellCompletions(args) => cmd::shell_completions::run(args).await,

        // Legacy aliases
        Command::LegacyRun(args) | Command::LegacyDaemon(args) => cmd::node::run::run(args, metrics.unwrap()).await,
        Command::LegacyBootstrap(args) => cmd::node::bootstrap::run(args).await,
        Command::LegacyResetToEpoch(args) => cmd::dev::ledger::reset::run(args).await,
        Command::LegacyCreateSnapshots(args) => cmd::snapshot::create::run(args).await,
        Command::LegacyDumpChainDB(args) => cmd::dev::chain::dump::run(args).await,
        Command::LegacyRemoveValidationStatus(args) => cmd::dev::chain::clear_invalid::run(args).await,
        Command::LegacyFetchChainHeaders(args) => cmd::dev::chain::fetch::run(args).await,
        Command::LegacyMigrateChainDB(args) => cmd::dev::chain::migrate::run(args).await,
        Command::LegacyRemoveChain(args) => cmd::dev::chain::remove::run(args).await,
        Command::LegacyDumpTracesSchema(args) => cmd::dev::traces::dump::run(args).await,
    };

    // TODO: we might also want to integrate this into a graceful shutdown system, and into a panic hook
    if let Err(report) = teardown() {
        eprintln!("Failed to teardown tracing: {report}");
    }

    result
}
