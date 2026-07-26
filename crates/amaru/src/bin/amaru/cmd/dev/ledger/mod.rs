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

pub(crate) mod convert;
#[cfg(feature = "mithril")]
pub(crate) mod mithril;
pub(crate) mod nonces;
pub(crate) mod reset;
pub(crate) mod states;
pub(crate) mod sync;

#[derive(Debug, Subcommand)]
pub(crate) enum LedgerCommand {
    /// Convert a Haskell cardano-node state for import into Amaru.
    Convert(convert::Args),

    /// Get or set nonces associated with a block in the chain store.
    #[command(subcommand)]
    Nonces(nonces::NoncesCommand),

    /// Manage ledger state snapshots.
    #[command(subcommand)]
    States(states::StatesCommand),

    /// Reset the ledger database to the beginning of a specific epoch.
    Reset(reset::Args),

    /// Sync the ledger with local blocks.
    ///
    /// The ledger state must be in an already bootstrapped state (e.g. via Amaru snapshots).
    Sync(sync::Args),

    #[cfg(feature = "mithril")]
    Mithril(mithril::Args),
}

impl LedgerCommand {
    pub(crate) fn into_runnable(self) -> Runnable {
        match self {
            Self::Convert(args) => convert::runnable(args),
            Self::Nonces(cmd) => cmd.into_runnable(),
            Self::States(cmd) => cmd.into_runnable(),
            Self::Reset(args) => reset::runnable(args),
            Self::Sync(args) => sync::runnable(args),
            #[cfg(feature = "mithril")]
            Self::Mithril(args) => mithril::runnable(args),
        }
    }
}
