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

//! Offline reset of the ledger database to the beginning of a stored epoch.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use amaru_ledger::state::MIN_LEDGER_SNAPSHOTS;
use anyhow::{Context, Result, bail};

/// Reset the ledger database at `ledger_dir` so that live state corresponds to the start of
/// `epoch` (by deleting later snapshots and copying epoch `epoch - 1` to `live/`).
///
/// This only mutates ledger snapshot directories. It does not touch the chain store.
pub fn reset_ledger_to_epoch(ledger_dir: &Path, epoch: amaru_kernel::Epoch) -> Result<()> {
    let folders = get_ledger_db_snapshots(ledger_dir)?;
    check_safe_to_reset(epoch, &folders)?;

    // Note: given the `check_safe_to_reset` above ensures we have epochs
    // we know penultimate_epoch will get reassigned
    let mut penultimate_epoch: Option<PathBuf> = None;
    for folder in folders {
        match folder.epoch {
            Epoch::Live => {
                fs::remove_dir_all(&folder.path)
                    .with_context(|| format!("failed to remove {}", folder.path.display()))?;
            }
            Epoch::Past(folder_epoch) => {
                if folder_epoch == epoch - 1 {
                    // set this aside to make a copy of it at the end
                    // if we were to copy right now, it could collide
                    // with the existing directory
                    penultimate_epoch = Some(folder.path);
                } else if folder_epoch >= epoch {
                    fs::remove_dir_all(&folder.path)
                        .with_context(|| format!("failed to remove {}", folder.path.display()))?;
                }
            }
        }
    }

    let penultimate_epoch = penultimate_epoch.unwrap_or_else(|| {
        unreachable!(
            "invariant violated: check_safe_to_reset should have guaranteed that penultimate_epoch gets assigned"
        );
    });

    copy_dir_recursive(&penultimate_epoch, &ledger_dir.join("live"))
        .with_context(|| format!("failed to copy {} to live/", penultimate_epoch.display()))?;

    Ok(())
}

#[derive(Clone, Copy, PartialEq)]
enum Epoch {
    Live,
    Past(amaru_kernel::Epoch),
}

impl Epoch {
    fn epoch_no(&self) -> Option<amaru_kernel::Epoch> {
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

fn get_ledger_db_snapshots(ledger_dir: &Path) -> Result<Vec<Folder>> {
    // The ledger db snapshots are organized as folders in ledger_dir
    // There's one folder for the "current" epoch, and one for each past epoch that's been saved
    Ok(fs::read_dir(ledger_dir)
        .with_context(|| format!("failed to read ledger_dir {}", ledger_dir.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|f| f.is_dir())
        .filter_map(|path| {
            let stem = path.file_stem()?.to_str()?;
            let epoch = if stem == "live" { Epoch::Live } else { Epoch::Past(stem.parse().ok()?) };
            Some(Folder { epoch, path })
        })
        .collect())
}

fn epoch_boundaries(folders: &[Folder]) -> Option<(amaru_kernel::Epoch, amaru_kernel::Epoch)> {
    let epoch_numbers = folders.iter().filter_map(|f| f.epoch.epoch_no());
    Some((epoch_numbers.clone().min()?, epoch_numbers.max()?))
}

fn check_safe_to_reset(epoch: amaru_kernel::Epoch, folders: &[Folder]) -> Result<()> {
    let (min_epoch, max_epoch) =
        epoch_boundaries(folders).ok_or_else(|| anyhow::anyhow!("no epochs to roll back to"))?;

    if epoch < min_epoch {
        bail!("cannot reset to an epoch that far in the past. We've only kept snapshots as far back as {}", min_epoch);
    }

    // The +1 here is because if we're resetting to 175, and the max epoch is 174,
    // we're *in* epoch 175, and we can just delete `live/` and copy `174` to `live`
    if epoch > max_epoch + 1 {
        bail!("cannot reset to an epoch in the future. We're currently in epoch {}", max_epoch + 1);
    }

    // We need MIN_LEDGER_SNAPSHOTS=3 previous epochs *plus* the "live" epoch, to function
    // so if we try to reset to the start of 165, but our earliest epoch is 163
    // this will break: we can keep 163 and 164, and copy 164 to live, but
    // that leaves us with only 2 epochs
    if epoch < min_epoch + MIN_LEDGER_SNAPSHOTS {
        bail!(
            "resetting to epoch {} would leave us with too few historical epochs to proceed. The earliest epoch you can reset to is {}",
            epoch,
            min_epoch + MIN_LEDGER_SNAPSHOTS,
        );
    }

    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> io::Result<()> {
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
