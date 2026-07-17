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

pub(crate) mod convert;
pub(crate) mod nonces;
pub(crate) mod reset;
pub(crate) mod states;

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
}
