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
use opentelemetry_sdk::metrics::SdkMeterProvider;

pub(crate) mod bootstrap;
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
}

impl NodeCommand {
    pub(crate) fn into_runnable(self, metrics: Option<SdkMeterProvider>) -> Runnable {
        match self {
            Self::Run(args) => run::runnable(args, metrics),
            Self::Bootstrap(args) => bootstrap::runnable(args),
        }
    }
}
