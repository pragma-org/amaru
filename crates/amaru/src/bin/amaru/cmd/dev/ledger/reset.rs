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

//! Low-level ledger-only epoch reset.
//!
//! Prefer [`amaru node rollback --epoch`](crate::cmd::node::rollback) which also realigns the
//! chain store and clears descendant validation flags.

use amaru::lifecycle::{Runnable, RuntimeKind};
use amaru_kernel::{Epoch, NetworkName};
use amaru_node::reset_ledger_to_epoch;
use amaru_observability::info;
use clap::Parser;

#[derive(Debug, Parser)]
pub struct Args {
    /// The epoch to reset to
    #[arg(
        value_name = amaru::value_names::UINT,
        env = amaru::env_vars::EPOCH,
    )]
    pub epoch: Epoch,

    #[command(flatten)]
    db_ledger: amaru::args::DbLedger,

    #[command(flatten)]
    network: amaru::args::Network,
}

pub(crate) fn runnable(args: Args) -> Runnable {
    Runnable::exit_on_signal(RuntimeKind::Simple, move || run(args))
}

async fn run(args: Args) -> anyhow::Result<()> {
    let network = NetworkName::from(args.network);
    let db_ledger = args.db_ledger.into_path_buf(network);

    info!(cli::dev::ledger::RESET, epoch = args.epoch, db_ledger = db_ledger.to_string_lossy(), network,);

    reset_ledger_to_epoch(&db_ledger, args.epoch)?;

    Ok(())
}
