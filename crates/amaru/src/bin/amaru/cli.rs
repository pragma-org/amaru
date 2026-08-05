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
    lifecycle::Runnable,
    observability::{Color, ObservabilityHints},
};
use amaru_kernel::GlobalParameters;
use amaru_tui as tui;
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};

use crate::cmd;

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Manage and operate the Amaru node.
    #[command(subcommand)]
    Node(cmd::node::NodeCommand),

    /// Manage bootstrap snapshots.
    #[command(subcommand)]
    Snapshot(cmd::snapshot::SnapshotCommand),

    /// Synchronize Amaru using Mithril snapshots.
    #[cfg(feature = "mithril")]
    #[command(subcommand)]
    Mithril(cmd::mithril::MithrilCommand),

    /// Developer and debugging tools.
    #[command(subcommand, hide = true)]
    Dev(cmd::dev::DevCommand),

    #[command(name = "shell-completions", hide = true)]
    ShellCompletions(cmd::shell_completions::Args),

    // Hidden backward-compatibility aliases for old top-level commands.
    #[command(hide = true, name = "run")]
    LegacyRun(cmd::node::run::Args),

    #[command(hide = true, name = "daemon")]
    LegacyDaemon(cmd::node::run::Args),

    #[command(hide = true, name = "bootstrap")]
    LegacyBootstrap(cmd::node::bootstrap::Args),

    /// Legacy alias for `amaru node rollback --epoch` (positional epoch for compatibility).
    #[command(hide = true, name = "reset-to-epoch")]
    LegacyResetToEpoch(cmd::dev::ledger::reset::Args),

    #[command(hide = true, name = "create-snapshots")]
    LegacyCreateSnapshots(cmd::snapshot::create::Args),

    #[command(hide = true, name = "dump-chain-db")]
    LegacyDumpChainDB(cmd::dev::chain::dump::Args),

    #[command(hide = true, name = "remove-validation-status")]
    LegacyRemoveValidationStatus(cmd::dev::chain::clear_invalid::Args),

    #[command(hide = true, name = "fetch-chain-headers")]
    LegacyFetchChainHeaders(cmd::dev::chain::fetch::Args),

    #[command(hide = true, name = "migrate-chain-db")]
    LegacyMigrateChainDB(cmd::dev::chain::migrate::Args),

    #[command(hide = true, name = "remove-chain")]
    LegacyRemoveChain(cmd::dev::chain::remove::Args),

    #[command(hide = true, name = "dump-traces-schema")]
    LegacyDumpTracesSchema(cmd::dev::traces::dump::Args),
}

impl Command {
    /// Collapse the clap command tree into a single [`Runnable`] leaf.
    ///
    /// The returned value describes the runtime and work factory only; observability must be
    /// set up on that runtime before [`amaru::lifecycle::Runnable::run_on`] is called.
    pub(crate) fn into_runnable(self) -> Runnable {
        match self {
            Command::Node(cmd) => cmd.into_runnable(),
            Command::Snapshot(cmd) => cmd.into_runnable(),
            #[cfg(feature = "mithril")]
            Command::Mithril(cmd) => cmd.into_runnable(),
            Command::Dev(cmd) => cmd.into_runnable(),
            Command::ShellCompletions(args) => cmd::shell_completions::runnable(args),
            // Legacy top-level aliases: same behaviour as their modern counterparts.
            Command::LegacyRun(args) | Command::LegacyDaemon(args) => cmd::node::run::runnable(args),
            Command::LegacyBootstrap(args) => cmd::node::bootstrap::runnable(args),
            Command::LegacyResetToEpoch(args) => {
                cmd::node::rollback::runnable_epoch(args.network, args.epoch, args.ledger_dir, None)
            }
            Command::LegacyCreateSnapshots(args) => cmd::snapshot::create::runnable(args),
            Command::LegacyDumpChainDB(args) => cmd::dev::chain::dump::runnable(args),
            Command::LegacyRemoveValidationStatus(args) => cmd::dev::chain::clear_invalid::runnable(args),
            Command::LegacyFetchChainHeaders(args) => cmd::dev::chain::fetch::runnable(args),
            Command::LegacyMigrateChainDB(args) => cmd::dev::chain::migrate::runnable(args),
            Command::LegacyRemoveChain(args) => cmd::dev::chain::remove::runnable(args),
            Command::LegacyDumpTracesSchema(args) => cmd::dev::traces::dump::runnable(args),
        }
    }

    #[allow(clippy::wildcard_enum_match_arm)]
    pub(crate) fn show_alternative_help(&self) -> Result<bool, Box<dyn std::error::Error>> {
        match self {
            Command::Node(cmd::node::NodeCommand::Run(args)) if args.help_global_parameters => {
                GlobalParameters::show_help()?;
                Ok(true)
            }
            Command::Node(cmd::node::NodeCommand::Bootstrap(args)) if args.help_global_parameters => {
                GlobalParameters::show_help()?;
                Ok(true)
            }
            Command::LegacyRun(args) | Command::LegacyDaemon(args) if args.help_global_parameters => {
                GlobalParameters::show_help()?;
                Ok(true)
            }
            Command::LegacyBootstrap(args) if args.help_global_parameters => {
                GlobalParameters::show_help()?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    pub(crate) fn skip_logging(&self) -> bool {
        matches!(
            self,
            Command::Dev(cmd::dev::DevCommand::Traces(cmd::dev::traces::TracesCommand::Dump(_)))
                | Command::Dev(cmd::dev::DevCommand::Traces(cmd::dev::traces::TracesCommand::Schema(_)))
                | Command::LegacyDumpTracesSchema(_)
                | Command::ShellCompletions(_)
        )
    }

    #[allow(clippy::wildcard_enum_match_arm)]
    pub(crate) fn tui_settings(&self) -> Option<tui::Settings> {
        match self {
            Command::Node(cmd::node::NodeCommand::Run(args))
            | Command::LegacyRun(args)
            | Command::LegacyDaemon(args) => Some(args.tui_settings()),
            _ => None,
        }
    }
}

impl ObservabilityHints for Command {
    fn listen_address(&self) -> Option<&str> {
        #[allow(clippy::wildcard_enum_match_arm)]
        match self {
            Command::Node(cmd::node::NodeCommand::Run(args)) => Some(args.listen_address()),
            Command::LegacyRun(args) | Command::LegacyDaemon(args) => Some(args.listen_address()),
            _ => None,
        }
    }
}

#[derive(Debug, Parser)]
#[clap(name = "Amaru")]
#[clap(bin_name = "amaru")]
#[clap(author, about, long_about = None)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,

    /// Control color output.
    #[clap(long, env = "AMARU_COLOR", default_value = "auto")]
    pub(crate) color: Color,

    /// Emit trace events as structured JSON instead of human-readable text.
    #[clap(long, action, env = "AMARU_WITH_JSON_TRACES")]
    pub(crate) with_json_traces: bool,

    /// Export traces and metrics via OpenTelemetry (OTLP).
    #[clap(long, action, env = "AMARU_WITH_OPEN_TELEMETRY")]
    pub(crate) with_open_telemetry: bool,
}

pub(crate) fn command(version: &'static str) -> clap::Command {
    <Cli as CommandFactory>::command().version(version)
}

pub(crate) fn parse(version: &'static str) -> Result<Cli, clap::Error> {
    let matches = <Cli as CommandFactory>::command()
        // NOTE: Hiding GlobalParameters options at 'runtime'
        //
        // Those options aren't declared hidden because it makes constructing the dedicated help a
        // lot harder. So we instead declare them visible and only hide them here.
        .mut_subcommand("node", |cmd| {
            cmd.mut_subcommand("run", GlobalParameters::hide_options)
                .mut_subcommand("bootstrap", GlobalParameters::hide_options)
        })
        // Also hide on legacy top-level aliases
        .mut_subcommand("run", GlobalParameters::hide_options)
        .mut_subcommand("daemon", GlobalParameters::hide_options)
        .mut_subcommand("bootstrap", GlobalParameters::hide_options)
        .version(version)
        .get_matches();

    let cli = <Cli as FromArgMatches>::from_arg_matches(&matches)?;

    Ok(cli)
}
