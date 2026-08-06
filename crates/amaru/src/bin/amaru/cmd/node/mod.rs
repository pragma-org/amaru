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

pub(crate) mod bootstrap;
pub(crate) mod rm;
pub(crate) mod rollback;
pub(crate) mod run;

#[derive(Debug, Subcommand)]
pub(crate) enum NodeCommand {
    /// Run the node in all its glory.
    #[command(alias = "daemon")]
    Run(run::Args),

    /// Bootstrap the node with needed data.
    ///
    /// This command simplifies the process of bootstrapping an Amaru node for any given well-known network:
    ///
    ///   - mainnet
    ///   - preprod
    ///   - preview
    ///
    /// It imports snapshots, bootstrap headers and bootstrap nonces in one step.
    #[clap(verbatim_doc_comment)]
    Bootstrap(bootstrap::Args),

    /// Roll the node databases back after a failure (for example a wrongly invalidated block).
    ///
    /// The node should be stopped before running this command.
    ///
    /// - `--immutable-tip` only realigns the chain store to the ledger's immutable tip. The ledger is
    ///   left unchanged (the volatile DB is not persisted, so offline it is already gone).
    ///
    /// - `--epoch` rewinds the ledger to the start of the given epoch (via historical snapshots) and
    ///   then realigns the chain store to the new ledger tip.
    ///
    /// Chain realignment sets the anchor and best tip to the target point, culls the best-chain
    /// fragment after it, and clears all block-validation flags on descendant headers so they can be
    /// re-validated on the next run. Headers and blocks themselves are kept.
    Rollback(rollback::Args),

    /// Remove the node's ledger and chain databases.
    Rm(rm::Args),
}

impl NodeCommand {
    pub(crate) fn into_runnable(self) -> Runnable {
        match self {
            Self::Run(args) => run::runnable(args),
            Self::Bootstrap(args) => bootstrap::runnable(args),
            Self::Rollback(args) => rollback::runnable(args),
            Self::Rm(args) => rm::runnable(args),
        }
    }
}
