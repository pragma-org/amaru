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

use std::path::PathBuf;

use amaru::{
    default_ledger_dir,
    lifecycle::{Runnable, RuntimeKind},
};
use amaru_kernel::{Epoch, NetworkName};
use amaru_node::reset_ledger_to_epoch;
use clap::Parser;
use tracing::info;

#[derive(Debug, Parser)]
pub struct Args {
    /// The epoch to reset to
    #[arg(
        value_name = amaru::value_names::UINT,
        env = amaru::env_vars::EPOCH,
    )]
    pub epoch: Epoch,

    /// The path to the ledger database to reset
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::LEDGER_DIR,
    )]
    pub ledger_dir: Option<PathBuf>,

    /// Network of the underlying chain database.
    #[arg(
        long,
        value_name = amaru::value_names::NETWORK,
        env = amaru::env_vars::NETWORK,
    )]
    pub network: NetworkName,
}

pub(crate) fn runnable(args: Args) -> Runnable {
    Runnable::exit_on_signal(RuntimeKind::Simple, move || run(args))
}

async fn run(args: Args) -> anyhow::Result<()> {
    let ledger_dir = args.ledger_dir.unwrap_or_else(|| default_ledger_dir(args.network).into());

    info!(
        _command = "dev ledger reset",
        epoch = %args.epoch,
        ledger_dir = %ledger_dir.to_string_lossy(),
        network = %args.network,
        hint = "prefer `amaru node rollback --epoch` which also realigns the chain store",
        "running",
    );

    reset_ledger_to_epoch(&ledger_dir, args.epoch)?;
    Ok(())
}
