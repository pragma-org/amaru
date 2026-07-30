// Copyright 2026 PRAGMA
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

use amaru::lifecycle::Runnable;
use clap::Subcommand;

pub(crate) mod chain;
pub(crate) mod ledger;
pub(crate) mod traces;

#[derive(Debug, Subcommand)]
pub(crate) enum DevCommand {
    /// Chain store operations.
    #[command(subcommand)]
    Chain(chain::ChainCommand),

    /// Ledger store operations.
    #[command(subcommand)]
    Ledger(ledger::LedgerCommand),

    /// Observability and trace operations.
    #[command(subcommand)]
    Traces(traces::TracesCommand),
}

impl DevCommand {
    pub(crate) fn into_runnable(self) -> Runnable {
        match self {
            Self::Chain(cmd) => cmd.into_runnable(),
            Self::Ledger(cmd) => cmd.into_runnable(),
            Self::Traces(cmd) => cmd.into_runnable(),
        }
    }
}
