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

pub(crate) mod ancestors;
pub(crate) mod best_chain;
pub(crate) mod children;
pub(crate) mod clear_invalid;
pub(crate) mod dump;
pub(crate) mod fetch;
pub(crate) mod migrate;
pub(crate) mod prune;
pub(crate) mod remove;

#[derive(Debug, Subcommand)]
pub(crate) enum ChainCommand {
    /// Walk back from a given point showing block height, presence, validation status and best chain flag.
    Ancestors(ancestors::Args),

    /// Show the best chain tip and computed best tip candidate.
    BestChain(best_chain::Args),

    /// Walk forward from a given point showing child tips with validation status.
    Children(children::Args),

    /// Clear the validation status of the given blocks from the chain database.
    ClearInvalid(clear_invalid::Args),

    /// Dump the content of the chain database for troubleshooting purposes.
    ///
    /// This command dumps the _whole_ content of the chain database in a human-readable format:
    ///  - Headers (hash + hex-encoded body)
    ///  - Parent-child relationships between headers
    ///  - Nonces
    ///  - Blocks
    ///  - Best chain anchor, tip and length
    Dump(dump::Args),

    /// Fetch specified chain headers.
    Fetch(fetch::Args),

    /// Migrate the chain database to the current version.
    ///
    /// This command is only relevant when one upgrades Amaru to a newer version that
    /// requires changes in the database format.
    Migrate(migrate::Args),

    /// Prune old chain data going backward, limited by available ledger states.
    Prune(prune::Args),

    /// Remove the given chain fragment from the chain database.
    Remove(remove::Args),
}

impl ChainCommand {
    pub(crate) fn into_runnable(self) -> Runnable {
        match self {
            Self::Ancestors(args) => ancestors::runnable(args),
            Self::BestChain(args) => best_chain::runnable(args),
            Self::Children(args) => children::runnable(args),
            Self::ClearInvalid(args) => clear_invalid::runnable(args),
            Self::Dump(args) => dump::runnable(args),
            Self::Fetch(args) => fetch::runnable(args),
            Self::Migrate(args) => migrate::runnable(args),
            Self::Prune(args) => prune::runnable(args),
            Self::Remove(args) => remove::runnable(args),
        }
    }
}
