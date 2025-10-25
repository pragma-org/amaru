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

use amaru_kernel::{EraHistory, Point, default_ledger_dir, network::NetworkName};
use amaru_ledger::{
    bootstrap::import_initial_snapshot,
    store::{EpochTransitionProgress, Store, TransactionalContext},
};
use amaru_progress_bar::new_terminal_progress_bar;
use amaru_stores::rocksdb::{RocksDB, RocksDbConfig};
use clap::Parser;
use std::{
    fs,
    path::{Path, PathBuf},
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
    /// Should be one of 'mainnet', 'preprod', 'preview' or 'testnet_<magic>' where
    /// `magic` is a 32-bits unsigned value denoting a particular testnet.
    #[arg(
        long,
        value_name = "NETWORK",
        env = "AMARU_NETWORK",
        default_value_t = NetworkName::Preprod,
    )]
    network: NetworkName,
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("malformed date: {}", .0)]
    MalformedDate(String),
    #[error("invalid snapshot file: {0}")]
    InvalidSnapshotFile(PathBuf),
    #[error(
        "You must provide either a single .cbor snapshot file (--snapshot) or a directory containing multiple .cbor snapshots (--snapshot-dir)"
    )]
    IncorrectUsage,
}

pub async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let ledger_dir = args
        .ledger_dir
        .unwrap_or_else(|| default_ledger_dir(args.network).into());
    if !args.snapshot.is_empty() {
        import_all(args.network, &args.snapshot, &ledger_dir).await
    } else if let Some(snapshot_dir) = args.snapshot_dir {
        import_all_from_directory(args.network, &ledger_dir, &snapshot_dir).await
    } else {
        Err(Error::IncorrectUsage.into())
    }
}

pub(crate) async fn import_all_from_directory(
    network: NetworkName,
    ledger_dir: &PathBuf,
    snapshot_dir: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut snapshots = fs::read_dir(snapshot_dir)?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().and_then(|s| s.to_str()) == Some("cbor"))
        .collect::<Vec<_>>();

    sort_snapshots_by_slot(&mut snapshots);

    import_all(network, &snapshots, ledger_dir).await
}

fn sort_snapshots_by_slot(snapshots: &mut [PathBuf]) {
    // Sort by parsed slot number from filename
    snapshots.sort_by_key(|path| {
        path.file_name()
            .and_then(|s| s.to_str())
            .and_then(|s| s.split('.').next())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(u64::MAX)
    });
}

pub async fn import_all(
    network: NetworkName,
    snapshots: &Vec<PathBuf>,
    ledger_dir: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("Importing {} snapshots", snapshots.len());
    for snapshot in snapshots {
        import_one(network, snapshot, ledger_dir).await?;
    }
    Ok(())
}

#[expect(clippy::unwrap_used)]
pub async fn import_one(
    network: NetworkName,
    snapshot: &PathBuf,
    ledger_dir: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("Importing snapshot {}", snapshot.display());
    let point = Point::try_from(
        snapshot
            .as_path()
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap(),
    )
    .map_err(Error::MalformedDate)?;

    fs::create_dir_all(ledger_dir)?;
    let db = RocksDB::empty(RocksDbConfig::new(ledger_dir.into()))?;
    let mut file = fs::File::open(snapshot)?;
    let dir = snapshot
        .parent()
        .ok_or(Error::InvalidSnapshotFile(snapshot.into()))?;

    let era_history = make_era_history(dir, &point, network)?;
    let epoch = import_initial_snapshot(
        &db,
        &mut file,
        &point,
        &era_history,
        network,
        new_terminal_progress_bar,
        None,
        true,
    )?;

    db.next_snapshot(epoch)?;

    let transaction = db.create_transaction();
    transaction.try_epoch_transition(None, Some(EpochTransitionProgress::SnapshotTaken))?;
    transaction.commit()?;

    info!("Imported snapshot for epoch {}", epoch);
    Ok(())
}

fn make_era_history(
    dir: &Path,
    point: &Point,
    network: NetworkName,
) -> Result<EraHistory, Box<dyn std::error::Error>> {
    match network {
        NetworkName::Testnet(_) => {
            let filename = format!("history.{}.{}.json", point.slot_or_default(), point.hash());
            let history_file = dir.join(filename);
            if !history_file.is_file() {
                return Err(
                    format!("cannot import testnet era history from {:?}", history_file).into(),
                );
            };

            Ok(serde_json::from_slice(&fs::read(&history_file)?)?)
        }
        NetworkName::Mainnet | NetworkName::Preprod | NetworkName::Preview => {
            Ok(<&EraHistory>::from(network).clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, str::FromStr};

    use crate::cmd::import_ledger_state::{make_era_history, sort_snapshots_by_slot};
    use amaru_kernel::{Hash, HeaderHash, Point, Slot, network::NetworkName};
    use amaru_slot_arithmetic::TimeMs;

    #[test]
    fn make_era_history_for_tesnet_given_file_exists() {
        let dir = PathBuf::from("tests/data/");
        let hash: HeaderHash =
            Hash::from_str("4df4505d862586f9e2c533c5fbb659f04402664db1b095aba969728abfb77301")
                .unwrap();
        let point = Point::Specific(56073562, hash.to_vec());

        let history = make_era_history(&dir, &point, NetworkName::Testnet(14))
            .expect("fail to make era history");

        assert_eq!(
            TimeMs::from(5100000000),
            history
                .slot_to_relative_time_unchecked_horizon(Slot::from(5100000))
                .unwrap()
        );
    }

    #[test]
    fn sort_snapshot_file_names_by_slot_number() {
        let mut paths = [
            PathBuf::from(
                "172786.932b9688167139cf4792e97ae4771b6dc762ad25752908cce7b24c2917847516.cbor",
            ),
            PathBuf::from(
                "259174.a07da7616822a1ccb4811e907b1f3a3c5274365908a241f4d5ffab2a69eb8802.cbor",
            ),
            PathBuf::from(
                "86392.1d38de4ffae6090c24151578d331b1021adb8f37d158011616db4d47d1704968.cbor",
            ),
        ];

        sort_snapshots_by_slot(&mut paths);

        assert_eq!(
            PathBuf::from(
                "86392.1d38de4ffae6090c24151578d331b1021adb8f37d158011616db4d47d1704968.cbor"
            ),
            paths[0]
        );
    }
}
