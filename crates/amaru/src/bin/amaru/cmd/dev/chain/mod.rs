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

use clap::Subcommand;

pub(crate) mod clear_invalid;
pub(crate) mod dump;
pub(crate) mod fetch;
pub(crate) mod migrate;
pub(crate) mod remove;

#[derive(Debug, Subcommand)]
pub(crate) enum ChainCommand {
    /// Dump the content of the chain database for troubleshooting purposes.
    ///
    /// This command dumps the _whole_ content of the chain database in a human-readable format:
    ///  - Headers (hash + hex-encoded body)
    ///  - Parent-child relationships between headers
    ///  - Nonces
    ///  - Blocks
    ///  - Best chain anchor, tip and length
    Dump(dump::Args),

    /// Clear the validation status of the given blocks from the chain database.
    ClearInvalid(clear_invalid::Args),

    /// Fetch specified chain headers.
    Fetch(fetch::Args),

    /// Migrate the chain database to the current version.
    ///
    /// This command is only relevant when one upgrades Amaru to a newer version that
    /// requires changes in the database format.
    Migrate(migrate::Args),

    /// Remove the given chain fragment from the chain database.
    Remove(remove::Args),
}
