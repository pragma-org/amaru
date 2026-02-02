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

use amaru::{DEFAULT_NETWORK, default_ledger_dir};
use amaru_kernel::NetworkName;
use amaru_ledger::state::MIN_LEDGER_SNAPSHOTS;
use clap::Parser;
use std::{fs, io, path::PathBuf};
use tracing::info;

#[derive(Debug, Parser)]
pub struct Args {
    /// The epoch to reset to
    #[arg(
        value_name = amaru::value_names::UINT,
        env = amaru::env_vars::EPOCH,
    )]
    epoch: u64,

    /// The path to the ledger database to reset
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::LEDGER_DIR,
    )]
    ledger_dir: Option<PathBuf>,

    /// Network of the underlying chain database.
    #[arg(
        long,
        value_name = amaru::value_names::NETWORK,
        env = amaru::env_vars::NETWORK,
        default_value_t = DEFAULT_NETWORK,
    )]
    network: NetworkName,
}

pub async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let ledger_dir = args
        .ledger_dir
        .unwrap_or_else(|| default_ledger_dir(args.network).into());

    info!(
        _command = "reset-to-epoch",
        epoch = %args.epoch,
        ledger_dir = %ledger_dir.to_string_lossy(),
        network = %args.network,
        "running",
    );

    let folders = get_ledger_db_snapshots(&ledger_dir)?;

    check_safe_to_reset(args.epoch, &folders)?;

    // Note: given the `check_safe_to_reset` above ensures we have epochs
    // we know pentultimate_epoch will get reassigned
    let mut pentultimate_epoch: Option<PathBuf> = None;
    for folder in folders {
        match folder.epoch {
            Epoch::Live => {
                fs::remove_dir_all(&folder.path)
                    .map_err(|e| format!("failed to remove {}: {}", folder.path.display(), e))?;
            }
            Epoch::Past(epoch) => {
                if epoch == args.epoch - 1 {
                    // set this aside to make a copy of it at the end
                    // if we were to copy right now, it could collide
                    // with the existing directory
                    pentultimate_epoch = Some(folder.path);
                } else if epoch >= args.epoch {
                    fs::remove_dir_all(&folder.path).map_err(|e| {
                        format!("failed to remove {}: {}", folder.path.display(), e)
                    })?;
                }
            }
        }
    }

    let pentultimate_epoch = pentultimate_epoch.unwrap_or_else(|| {
        unreachable!("invariant violated: check_safe_to_reset should have guaranteed that pentultimate_epoch gets assigned");
    });

    copy_dir_recursive(&pentultimate_epoch, &ledger_dir.join("live"))?;

    Ok(())
}

#[derive(Clone, Copy, PartialEq)]
enum Epoch {
    Live,
    Past(u64),
}

impl Epoch {
    fn epoch_no(&self) -> Option<u64> {
        match self {
            Epoch::Live => None,
            Epoch::Past(e) => Some(*e),
        }
    }
}

#[derive(Clone)]
struct Folder {
    epoch: Epoch,
    path: PathBuf,
}

fn get_ledger_db_snapshots(
    ledger_dir: &PathBuf,
) -> Result<Vec<Folder>, Box<dyn std::error::Error>> {
    // The ledger db snapshots are organized as folders in ledger_dir
    // There's one folder for the "current" epoch, and one for each past epoch that's been saved
    Ok(fs::read_dir(ledger_dir)
        .map_err(|e| format!("failed to read ledger_dir {}: {}", ledger_dir.display(), e))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|f| f.is_dir())
        .filter_map(|path| {
            let stem = path.file_stem()?.to_str()?;
            let epoch = if stem == "live" {
                Epoch::Live
            } else {
                Epoch::Past(stem.parse::<u64>().ok()?)
            };
            Some(Folder { epoch, path })
        })
        .collect())
}

fn epoch_boundaries(folders: &[Folder]) -> Option<(u64, u64)> {
    let epoch_numbers = folders.iter().filter_map(|f| f.epoch.epoch_no());
    Some((epoch_numbers.clone().min()?, epoch_numbers.max()?))
}

fn check_safe_to_reset(epoch: u64, folders: &[Folder]) -> Result<(), Box<dyn std::error::Error>> {
    let (min_epoch, max_epoch) = epoch_boundaries(folders).ok_or("no epochs to roll back to")?;

    if epoch < min_epoch {
        return Err(format!("cannot reset to an epoch that far in the past. We've only kept snapshots as far back as {}", min_epoch).into());
    }

    // The +1 here is because if we're resetting to 175, and the max epoch is 174,
    // we're *in* epoch 175, and we can just delete `live/` and copy `174` to `live`
    if epoch > max_epoch + 1 {
        return Err(format!(
            "cannot reset to an epoch in the future. We're currently in epoch {}",
            max_epoch + 1
        )
        .into());
    }

    // We need MIN_LEDGER_SNAPSHOTS=3 previous epochs *plus* the "live" epoch, to function
    // so if we try to reset to the start of 165, but our earliest epoch is 163
    // this will break: we can keep 163 and 164, and copy 164 to live, but
    // that leaves us with only 2 epochs
    if epoch < min_epoch + MIN_LEDGER_SNAPSHOTS {
        return Err(format!(
            "resetting to epoch {} would leave us with too few historical epochs to proceed. The earliest epoch you can reset to is {}",
            epoch,
            min_epoch + MIN_LEDGER_SNAPSHOTS,
        ).into());
    }

    Ok(())
}

fn copy_dir_recursive(src: &PathBuf, dst: &PathBuf) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}
