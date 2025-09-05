// Copyright 2025 PRAGMA
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

use amaru_kernel::cbor;
use clap::Parser;
use std::path::PathBuf;
use tokio::fs::{self};

#[derive(Debug, Parser)]
pub struct Args {
    /// Path to the CBOR encoded ledger state snapshot as serialised by Haskell
    /// node.
    #[arg(long, value_name = "SNAPSHOT", verbatim_doc_comment, num_args(0..))]
    snapshot: Vec<PathBuf>,

    /// Directory to store converted snapshots into.
    ///
    /// Directory will be created if it does not exist.
    #[arg(long, value_name = "DIR", verbatim_doc_comment, num_args(0..))]
    target_dir: Option<PathBuf>,
}

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("Snapshot {0} does not exist")]
    SnapshotDoesNotExist(PathBuf),
    #[error(
        "Snapshot name {0} is not well-formed, expected something like 'xxxx.<epoch no>.<slot no>.<hash>'"
    )]
    InvalidSnapshotPath(PathBuf),
}

pub(crate) async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let target_dir = args.target_dir.unwrap_or(PathBuf::from("."));
    for snapshot in args.snapshot {
        convert_one_snapshot_file(&target_dir, &snapshot).await?;
    }

    Ok(())
}

async fn convert_one_snapshot_file(
    target_dir: &PathBuf,
    snapshot: &PathBuf,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if !snapshot.exists() {
        return Err(Box::new(Error::SnapshotDoesNotExist(snapshot.clone())));
    }

    let target_name = make_target_name(snapshot)?;
    let target_path = target_dir.join(target_name);
    fs::create_dir_all(target_path.parent().unwrap()).await?;

    convert_snapshot_to(snapshot, &target_path).await?;

    Ok(target_path)
}

async fn convert_snapshot_to(
    snapshot: &PathBuf,
    target_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = fs::read(snapshot).await?;
    let mut d = cbor::Decoder::new(bytes.as_slice());

    d.array()?;

    // version
    // https://github.com/abailly/ouroboros-consensus/blob/1508638f832772d21874e18e48b908fcb791cd49/ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/Util/Versioned.hs#L95
    d.skip()?;

    // ext ledger state
    // https://github.com/abailly/ouroboros-consensus/blob/1508638f832772d21874e18e48b908fcb791cd49/ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/Ledger/Extended.hs#L232
    d.array()?;

    // ledger state
    d.array()?;

    // there are 6 pair of integer triples before the actual ledger state I have no clue what they are
    d.skip()?;
    d.skip()?;
    d.skip()?;
    d.skip()?;
    d.skip()?;
    d.skip()?;

    // ???
    d.array()?;
    d.skip()?;

    // ledger state
    // https://github.com/abailly/ouroboros-consensus/blob/1508638f832772d21874e18e48b908fcb791cd49/ouroboros-consensus-cardano/src/shelley/Ouroboros/Consensus/Shelley/Ledger/Ledger.hs#L736
    d.array()?;

    // encoding version (2)
    d.u8()?;

    d.array()?;
    // tip
    d.skip()?;

    let p = d.position();
    fs::write(&target_path, &bytes[p..bytes.len() - 1]).await?;

    Ok(())
}

fn make_target_name(snapshot: &PathBuf) -> Result<String, Box<dyn std::error::Error>> {
    snapshot
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.split('.').collect())
        .and_then(|parts: Vec<&str>| {
            let slot = parts.get(3);
            let hash = parts.get(4);
            slot.and_then(|s| hash.map(|h| s.to_string() + "." + h + ".cbor"))
        })
        .ok_or(Box::new(Error::InvalidSnapshotPath(snapshot.clone())))
}

#[cfg(test)]
mod test {
    use amaru_kernel::network::NetworkName;
    use tokio::fs;

    use crate::cmd::import_ledger_state::import_one;

    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn fails_if_file_does_not_exist() {
        let tempdir = tempfile::tempdir().unwrap();
        let snapshot_path = PathBuf::from("does-not-exist");

        let result = convert_one_snapshot_file(&tempdir.path().into(), &snapshot_path).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn generates_converted_snapshot_in_given_target_dir() {
        let tempdir = tempfile::tempdir().unwrap();
        let snapshot_path = "tests/data/convert/ledger.snapshot.1.86392.1d38de4ffae6090c24151578d331b1021adb8f37d158011616db4d47d1704968".into();
        let expected_path = tempdir
            .path()
            .join("86392.1d38de4ffae6090c24151578d331b1021adb8f37d158011616db4d47d1704968.cbor");

        let result = convert_one_snapshot_file(&tempdir.path().to_path_buf(), &snapshot_path)
            .await
            .unwrap();

        let tmp_ledger_dir = tempdir.path().join("ledger.db");

        assert_eq!(expected_path, result.as_path());
        // for error reporting
        let dir = dir_content(&tempdir.path().to_path_buf()).await.unwrap();
        assert!(result.exists(), "target dir content: {:?}", dir);
        import_one(NetworkName::Testnet(42), &result, &tmp_ledger_dir)
            .await
            .expect(format!("fail to import snapshot from {result:?}").as_str());
    }

    async fn dir_content(path: &PathBuf) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let mut result = Vec::new();
        let mut acc: Vec<PathBuf> = Vec::new();
        acc.push(path.clone());
        while let Some(dir) = acc.pop() {
            let mut list = fs::read_dir(dir).await?;
            while let Some(entry) = list.next_entry().await? {
                let path = entry.path();
                result.push(path.clone());
                if path.is_dir() {
                    acc.push(path.clone());
                }
            }
        }
        Ok(result.iter().map(|p| format!("{}", p.display())).collect())
    }
}
