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

pub(crate) mod dump;

#[derive(Debug, Subcommand)]
pub(crate) enum TracesCommand {
    /// Dump all registered trace schemas as JSON Schema.
    ///
    /// This command outputs all registered trace schemas in JSON Schema format.
    /// Useful for documentation, tooling, and validation.
    Dump(dump::Args),

    /// Get the traces JSON schema (alias for dump).
    Schema(dump::Args),
}

impl TracesCommand {
    pub(crate) fn into_runnable(self) -> Runnable {
        match self {
            Self::Dump(args) | Self::Schema(args) => dump::runnable(args),
        }
    }
}
