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

use amaru_consensus::Nonces;
use amaru_kernel::{Epoch, HEADER_HASH_SIZE, Hash, Nonce, cbor};
use clap::Parser;
use std::path::{Path, PathBuf};
use tokio::fs::{self};

#[derive(Debug, Parser)]
pub struct Args {
    /// Path to the CBOR encoded ledger state snapshot as serialised by Haskell
    /// node.
    #[arg(long, value_name = "SNAPSHOT", verbatim_doc_comment, num_args(0..))]
    snapshot: Vec<PathBuf>,

    /// Directory to store converted snapshots into.
    ///
    /// Directory will be created if it does not exist, defaults to '.'.
    #[arg(long, value_name = "DIR", verbatim_doc_comment)]
    target_dir: Option<PathBuf>,
}

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("Snapshot {0} does not exist")]
    SnapshotDoesNotExist(PathBuf),
    #[error("Snapshot {0} is not a file")]
    SnapshotIsNotFile(PathBuf),
}

pub(crate) async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let target_dir = args.target_dir.unwrap_or(PathBuf::from("."));
    for snapshot in args.snapshot {
        convert_one_snapshot_file(&target_dir, &snapshot).await?;
    }

    Ok(())
}

async fn convert_one_snapshot_file(
    target_dir: &Path,
    snapshot: &PathBuf,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if !snapshot.exists() {
        return Err(Box::new(Error::SnapshotDoesNotExist(snapshot.clone())));
    }
    if !snapshot.is_file() {
        return Err(Box::new(Error::SnapshotIsNotFile(snapshot.clone())));
    }

    fs::create_dir_all(target_dir).await?;

    convert_snapshot_to(snapshot, target_dir).await
}

async fn convert_snapshot_to(
    snapshot: &PathBuf,
    target_dir: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
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

    // HardForkCombinator telescope encoding.
    // encodes the era history. There are 6 eras before conway.
    // FIXME: pass the current Era to know how many skips to do
    d.skip()?;
    d.skip()?;
    d.skip()?;
    d.skip()?;
    d.skip()?;
    d.skip()?;

    // conway era bounds
    d.array()?;
    d.skip()?;

    // ledger state
    // https://github.com/abailly/ouroboros-consensus/blob/1508638f832772d21874e18e48b908fcb791cd49/ouroboros-consensus-cardano/src/shelley/Ouroboros/Consensus/Shelley/Ledger/Ledger.hs#L736
    d.array()?;

    // encoding version (2)
    d.u8()?;

    d.array()?;
    // tip
    // https://github.com/abailly/ouroboros-consensus/blob/1508638f832772d21874e18e48b908fcb791cd49/ouroboros-consensus-cardano/src/shelley/Ouroboros/Consensus/Shelley/Ledger/Ledger.hs#L694
    // the Tip is wrapped in a WithOrigin type hence the double array
    d.array()?;
    d.array()?;
    let slot = d.u64()?;
    let _height = d.u64()?;
    let hash: Hash<HEADER_HASH_SIZE> = d.decode()?;

    // ledger state
    let begin = d.position();
    d.skip()?;
    let end = d.position();

    // shelley transition:
    // https://github.com/abailly/ouroboros-consensus/blob/1508638f832772d21874e18e48b908fcb791cd49/ouroboros-consensus-cardano/src/shelley/Ouroboros/Consensus/Shelley/Ledger/Ledger.hs#L717
    d.skip()?;

    // header state
    // https://github.com/abailly/ouroboros-consensus/blob/1508638f832772d21874e18e48b908fcb791cd49/ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/HeaderValidation.hs#L639
    d.array()?;
    // the Tip is wrapped in a WithOrigin type hence the double array
    d.array()?;
    d.array()?;
    // ? Number of eras ?
    d.u8()?;

    // AnnTip
    // https://github.com/abailly/ouroboros-consensus/blob/1508638f832772d21874e18e48b908fcb791cd49/ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/HeaderValidation.hs#L599
    d.array()?;
    // NOTE: The encoding of an AnnTip is not consistent with the encoding of a Tip
    let _tip_slot = d.u64()?;
    let _tip_hash: Hash<HEADER_HASH_SIZE> = d.decode()?;
    let _tip_height = d.u64()?;

    // ChainDepState for Praos
    d.array()?;

    // more HFC telescope encoding
    d.skip()?;
    d.skip()?;
    d.skip()?;
    d.skip()?;
    d.skip()?;
    d.skip()?;

    // the actual PraosState
    // https://github.com/abailly/ouroboros-consensus/blob/1508638f832772d21874e18e48b908fcb791cd49/ouroboros-consensus-protocol/src/ouroboros-consensus-protocol/Ouroboros/Consensus/Protocol/Praos.hs#L280
    d.array()?;
    // HFC era bounds?
    d.skip()?;

    // version
    // https://github.com/abailly/ouroboros-consensus/blob/1508638f832772d21874e18e48b908fcb791cd49/ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/Util/Versioned.hs#L95
    d.array()?;
    // currently 0?
    d.u8()?;
    d.array()?;

    // last slot
    d.array()?;
    // ?
    d.u8()?;
    d.u64()?;
    // TODO: ocert counters
    // they are currently in the ledger, not sure where they come from
    d.skip()?;

    // each nonce is a sum type hence encoded as an array, why?
    // https://github.com/input-output-hk/cardano-ledger/blob/12acadc5bf352d8a5ad3c9b982204207278cdc90/libs/cardano-ledger-core/src/Cardano/Ledger/BaseTypes.hs#L493
    d.array()?;
    d.u8()?;
    let evolving: Nonce = d.decode()?;

    d.array()?;
    d.u8()?;
    let candidate: Nonce = d.decode()?;

    d.array()?;
    d.u8()?;
    let active: Nonce = d.decode()?;

    d.array()?;
    d.u8()?;
    let tail = d.decode()?;

    // previous epoch nonce, can be defined or 0
    // FIXME: do we use it?
    d.skip()?;

    let nonces = Nonces {
        active,
        evolving,
        candidate,
        epoch: Epoch::from(0), // FIXME: should be computed from era history
        tail,
    };

    write_nonces(target_dir, nonces).await?;
    write_ledger_snapshot(target_dir, slot, hash, &bytes[begin..end]).await
}

async fn write_nonces(target_dir: &Path, nonces: Nonces) -> Result<(), Box<dyn std::error::Error>> {
    let target_path = target_dir.join(format!("nonces.{}.json", nonces.epoch));
    fs::write(&target_path, serde_json::to_string(&nonces)?).await?;
    Ok(())
}

async fn write_ledger_snapshot(
    target_dir: &Path,
    slot: u64,
    hash: Hash<HEADER_HASH_SIZE>,
    ledger_data: &[u8],
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let target_path = target_dir.join(format!("{}.{}.cbor", slot, hash));
    fs::write(&target_path, ledger_data).await?;
    Ok(target_path)
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::cmd::import_ledger_state::import_all;
    use amaru_kernel::network::NetworkName;
    use std::path::PathBuf;
    use tokio::fs;

    #[tokio::test]
    async fn fails_if_file_does_not_exist() {
        let tempdir = tempfile::tempdir().unwrap();
        let snapshot_path = PathBuf::from("does-not-exist");

        let result = convert_one_snapshot_file(tempdir.path(), &snapshot_path).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn generates_converted_snapshots_in_given_target_dir() {
        let tempdir = tempfile::tempdir().unwrap();
        let expected_paths = vec![
            tempdir.path().join(
                "86392.1d38de4ffae6090c24151578d331b1021adb8f37d158011616db4d47d1704968.cbor",
            ),
            tempdir.path().join(
                "172786.932b9688167139cf4792e97ae4771b6dc762ad25752908cce7b24c2917847516.cbor",
            ),
            tempdir.path().join(
                "259174.a07da7616822a1ccb4811e907b1f3a3c5274365908a241f4d5ffab2a69eb8802.cbor",
            ),
        ];

        let args = super::Args {
            snapshot: dir_content(Path::new("tests/data/convert")).await.unwrap(),
            target_dir: Some(tempdir.path().to_path_buf()),
        };

        run(args)
            .await
            .expect("unexpected error in conversion test");

        assert!(
            expected_paths.iter().all(|p| p.exists()),
            "missing converted snapshots in {:?}",
            dir_content(tempdir.path())
                .await
                .unwrap_or_else(|_| panic!("failed to list {tempdir:?} content"))
        );

        assert_import_ledger_db(&expected_paths, &tempdir.path().join("ledger.db")).await;
    }

    async fn assert_import_ledger_db(expected_paths: &Vec<PathBuf>, ledger_dir: &PathBuf) {
        import_all(NetworkName::Testnet(42), expected_paths, ledger_dir)
            .await
            .unwrap_or_else(|_| panic!("fail to import snapshots {expected_paths:?}"));
    }

    #[tokio::test]
    async fn run_produces_nonces_json_file() {
        let tempdir = tempfile::tempdir().unwrap();
        let target_dir = tempdir.path().to_path_buf();

        let args = super::Args {
            snapshot: dir_content(Path::new("tests/data/convert")).await.unwrap(),
            target_dir: Some(target_dir.clone()),
        };

        run(args)
            .await
            .expect("unexpected error in conversion test");

        let nonces_json_path = target_dir.join("nonces.0.json");
        assert!(
            nonces_json_path.exists(),
            "nonces.json file should be created in target directory"
        );
    }

    async fn dir_content(path: &Path) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
        let mut result = Vec::new();
        let mut acc: Vec<PathBuf> = Vec::new();
        acc.push(path.to_path_buf());
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
        Ok(result)
    }
}
