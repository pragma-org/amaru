// Copyright 2024 PRAGMA
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

use amaru_kernel::{
    network::NetworkName, EraHistory, parse_point, default_ledger_dir,
};
use amaru_ledger::{
    self,
    state::{self},
    store::{
        EpochTransitionProgress, Store, TransactionalContext,
    },
};
use amaru_stores::rocksdb::RocksDB;
use clap::Parser;
use std::{
    collections::{BTreeSet},
    fs, iter,
    path::PathBuf,
};
use tracing::info;

#[derive(Debug, Parser)]
pub struct Args {
    /// Path to the CBOR snapshot. The snapshot can be obtained from the Haskell
    /// cardano-node, using the `DebugEpochState` command, serialised as CBOR.
    ///
    /// The snapshot must be named after the point on-chain it is reflecting, as
    ///
    /// `  {SLOT}.{BLOCK_HEADER_HASH}.cbor`
    ///
    /// For example:
    ///
    ///   68774372.36f5b4a370c22fd4a5c870248f26ac72c0ac0ecc34a42e28ced1a4e15136efa4.cbor
    ///
    /// Can be repeated multiple times for multiple snapshots.
    #[arg(long, value_name = "SNAPSHOT", verbatim_doc_comment, num_args(0..))]
    snapshot: Vec<PathBuf>,
    /// Path to a directory containing multiple CBOR snapshots to import.
    #[arg(long, value_name = "DIR")]
    snapshot_dir: Option<PathBuf>,

    /// Path of the ledger on-disk storage.
    #[arg(long, value_name = "DIR")]
    ledger_dir: Option<PathBuf>,

    /// Network the snapshots are imported from.
    ///
    /// Should be one of 'mainnet', 'preprod', 'preview' or 'testnet:<magic>' where
    /// `magic` is a 32-bits unsigned value denoting a particular testnet.
    #[arg(
        long,
        value_name = "NETWORK",
        default_value_t = NetworkName::Preprod,
    )]
    network: NetworkName,
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("malformed date: {}", .0)]
    MalformedDate(String),
    #[error("You must provide either a single .cbor snapshot file (--snapshot) or a directory containing multiple .cbor snapshots (--snapshot-dir)")]
    IncorrectUsage,
}

pub async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let network = args.network;
    let era_history = network.into();
    let ledger_dir = args
        .ledger_dir
        .unwrap_or_else(|| default_ledger_dir(args.network).into());
    if !args.snapshot.is_empty() {
        import_all(&args.snapshot, &ledger_dir, era_history).await
    } else if let Some(snapshot_dir) = args.snapshot_dir {
        import_all_from_directory(&ledger_dir, era_history, &snapshot_dir).await
    } else {
        Err(Error::IncorrectUsage.into())
    }
}

pub(crate) async fn import_all_from_directory(
    ledger_dir: &PathBuf,
    era_history: &EraHistory,
    snapshot_dir: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut snapshots = fs::read_dir(snapshot_dir)?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().and_then(|s| s.to_str()) == Some("cbor"))
        .collect::<Vec<_>>();

    sort_snapshots_by_slot(&mut snapshots);

    import_all(&snapshots, ledger_dir, era_history).await
}

fn sort_snapshots_by_slot(snapshots: &mut [PathBuf]) {
    // Sort by parsed slot number from filename
    snapshots.sort_by_key(|path| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.split('.').next())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(u64::MAX)
    });

    snapshots.sort();
}

async fn import_all(
    snapshots: &Vec<PathBuf>,
    ledger_dir: &PathBuf,
    era_history: &EraHistory,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("Importing {} snapshots", snapshots.len());
    for snapshot in snapshots {
        import_one(snapshot, ledger_dir, era_history).await?;
    }
    Ok(())
}

#[allow(clippy::unwrap_used)]
async fn import_one(
    snapshot: &PathBuf,
    ledger_dir: &PathBuf,
    era_history: &EraHistory,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("Importing snapshot {}", snapshot.display());
    let point = parse_point(
        snapshot
            .as_path()
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap(),
    )
    .map_err(Error::MalformedDate)?;

    fs::create_dir_all(ledger_dir)?;
    let db = RocksDB::empty(ledger_dir, era_history)?;
    let bytes = fs::read(snapshot)?;

    let epoch = amaru_stores::decode_new_epoch_state(&db, &bytes, &point, era_history, &None, true)?;
    let transaction = db.create_transaction();
    transaction.save(
        &point,
        None,
        Default::default(),
        Default::default(),
        iter::empty(),
        BTreeSet::new(),
    )?;
    transaction.commit()?;

    db.next_snapshot(epoch)?;

    let transaction = db.create_transaction();
    transaction.try_epoch_transition(None, Some(EpochTransitionProgress::SnapshotTaken))?;
    transaction.commit()?;

    info!("Imported snapshot for epoch {}", epoch);
    Ok(())
}
