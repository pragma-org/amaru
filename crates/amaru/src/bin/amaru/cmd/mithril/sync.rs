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

use std::{path::PathBuf, sync::Arc};

use amaru::lifecycle::{Runnable, RuntimeKind};
use amaru_kernel::{NetworkName, Slot};
use amaru_node::{DefaultMithrilObserver, MithrilCancellation, MithrilSyncError, MithrilSynchronizer};

#[derive(Debug, clap::Parser)]
pub(crate) struct Args {
    #[command(flatten)]
    network: amaru::args::Network,

    #[command(flatten)]
    db_ledger: amaru::args::DbLedger,

    #[command(flatten)]
    db_chain: amaru::args::DbChain,

    /// Path of the Mithril snapshots on-disk storage.
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        default_value = "mithril-snapshots",
        env = amaru::env_vars::MITHRIL_SNAPSHOTS,
        alias = "snapshots-dir",
        verbatim_doc_comment
    )]
    snapshots: PathBuf,

    /// Ingest blocks until (and including) the given slot.
    /// If not provided, will ingest all available blocks.
    #[arg(
        long,
        value_name = amaru::value_names::SLOT,
        env = amaru::env_vars::MITHRIL_UNTIL_SLOT,
        alias = "ingest-until-slot",
    )]
    until_slot: Option<Slot>,

    /// Ingest at most the given number of blocks.
    /// If not provided, will ingest all available blocks.
    #[arg(
        long,
        value_name = amaru::value_names::UINT,
        env = amaru::env_vars::MITHRIL_MAX_BLOCKS,
        alias = "ingest-maximum-blocks",
    )]
    max_blocks: Option<usize>,
}

pub(crate) fn runnable(args: Args) -> Runnable {
    Runnable::soft(RuntimeKind::Io, move |shutdown, _meter| run(args, shutdown.token()))
}

async fn run(args: Args, cancellation: MithrilCancellation) -> anyhow::Result<()> {
    let Args { network, db_ledger, db_chain, snapshots, until_slot, max_blocks } = args;
    let network = NetworkName::from(network);
    let db_ledger = db_ledger.into_path_buf(network);
    let db_chain = db_chain.into_path_buf(network);
    let synchronizer =
        MithrilSynchronizer::new(network, db_ledger, db_chain, snapshots).ingest_limits(until_slot, max_blocks);
    match synchronizer.synchronize(cancellation, Arc::new(DefaultMithrilObserver::new())).await {
        Ok(_) | Err(MithrilSyncError::Cancelled) => Ok(()),
        Err(error) => Err(error.into()),
    }
}
