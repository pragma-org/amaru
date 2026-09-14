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

use std::{
    fs::{self, File, TryLockError},
    path::{Path, PathBuf},
};

use amaru::{
    default_chain_dir, default_ledger_dir,
    lifecycle::{Runnable, RuntimeKind},
};
use amaru_kernel::NetworkName;

use super::{download, ingest};

#[derive(Debug, clap::Parser)]
pub(crate) struct Args {
    /// The target network to choose from.
    ///
    /// Should be one of 'mainnet', 'preprod', 'preview' or `testnet:<magic>` where
    /// `magic` is a 32-bits unsigned value denoting a particular testnet.
    #[arg(
        long,
        value_name = "NETWORK_NAME",
        env = "AMARU_NETWORK",
        default_value_t = NetworkName::Preprod,
        verbatim_doc_comment
    )]
    network: NetworkName,

    /// Path of the ledger on-disk storage.
    #[arg(long, value_name = "DIR", env = "AMARU_LEDGER_DIR")]
    ledger_dir: Option<PathBuf>,

    /// Path of the chain on-disk storage.
    #[arg(long, value_name = "DIR", env = "AMARU_CHAIN_DIR")]
    chain_dir: Option<PathBuf>,

    /// Path of the Mithril snapshots on-disk storage.
    #[arg(
        long,
        value_name = "DIR",
        default_value = "mithril-snapshots",
        env = "AMARU_MITHRIL_SNAPSHOTS_DIR",
        verbatim_doc_comment
    )]
    snapshots_dir: PathBuf,

    /// Ingest blocks until (and including) the given slot.
    /// If not provided, will ingest all available blocks.
    #[arg(long, value_name = "SLOT", env = "AMARU_INGEST_UNTIL_SLOT")]
    ingest_until_slot: Option<u64>,

    /// Ingest at most the given number of blocks.
    /// If not provided, will ingest all available blocks.
    #[arg(long, value_name = "INT", env = "AMARU_INGEST_MAXIMUM_BLOCKS")]
    ingest_maximum_blocks: Option<usize>,
}

pub(crate) fn runnable(args: Args) -> Runnable {
    Runnable::exit_on_signal(RuntimeKind::Io, move || run(args))
}

fn acquire_sync_lock(snapshots_dir: &Path, network: NetworkName) -> anyhow::Result<File> {
    let target_dir = snapshots_dir.join(network.to_string());
    fs::create_dir_all(&target_dir)?;
    let lock_path = target_dir.join(".sync.lock");
    let lock = File::create(&lock_path)?;
    match lock.try_lock() {
        Ok(()) => Ok(lock),
        Err(TryLockError::WouldBlock) => Err(anyhow::anyhow!("another Mithril sync is using {}", target_dir.display())),
        Err(error) => Err(error.into()),
    }
}

async fn run(args: Args) -> anyhow::Result<()> {
    let Args { network, ledger_dir, chain_dir, snapshots_dir, ingest_until_slot, ingest_maximum_blocks } = args;
    let ledger_dir = ledger_dir.unwrap_or_else(|| default_ledger_dir(network).into());
    let chain_dir = chain_dir.unwrap_or_else(|| default_chain_dir(network).into());
    let _sync_lock = acquire_sync_lock(&snapshots_dir, network)?;

    let immutable_dir = download::run(network, &ledger_dir, &snapshots_dir).await?;
    ingest::run(network, ledger_dir, chain_dir, immutable_dir, ingest_until_slot, ingest_maximum_blocks).await
}
