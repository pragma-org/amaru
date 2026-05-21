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
    fs,
    io::{self, Cursor},
    path::{Path, PathBuf},
};

use flate2::{Compression, GzBuilder};
use tar::{Builder, Header};

use super::EpochTarget;

pub(super) fn snapshot_path_for_target(snapshot_root: &Path, target: &EpochTarget) -> PathBuf {
    snapshot_root.join(format!("{}.{}", target.slot, target.hash))
}

pub(super) fn archive_path_for_target(snapshot_root: &Path, target: &EpochTarget) -> PathBuf {
    snapshot_root.join(format!("{}.{}.tar.gz", target.slot, target.hash))
}

pub(super) fn existing_snapshot_paths(snapshot_root: &Path, targets: &[EpochTarget]) -> Vec<PathBuf> {
    targets.iter().map(|target| snapshot_path_for_target(snapshot_root, target)).filter(|path| path.exists()).collect()
}

pub(super) fn existing_archive_paths(snapshot_root: &Path, targets: &[EpochTarget]) -> Vec<PathBuf> {
    targets.iter().map(|target| archive_path_for_target(snapshot_root, target)).filter(|path| path.is_file()).collect()
}

fn metadata_path_for_epoch(metadata_dir: &Path, epoch: u64) -> PathBuf {
    metadata_dir.join(format!("{epoch}.json"))
}

pub(super) fn write_epoch_metadata(
    metadata_dir: &Path,
    target: &EpochTarget,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = metadata_path_for_epoch(metadata_dir, target.epoch);
    fs::write(path, serde_json::to_vec_pretty(target)?)?;
    Ok(())
}

pub(super) fn materialize_snapshot(
    snapshot_dir: &Path,
    snapshot_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let tmp_path = temporary_snapshot_path(snapshot_path)?;

    if tmp_path.exists() {
        fs::remove_dir_all(&tmp_path)?;
    }

    if snapshot_path.exists() {
        return Err(format!("refusing to overwrite existing snapshot directory {}", snapshot_path.display()).into());
    }

    fs::create_dir_all(&tmp_path)?;
    copy_snapshot_contents(snapshot_dir, &tmp_path)?;
    fs::rename(tmp_path, snapshot_path)?;

    Ok(())
}

pub(super) fn write_snapshot_archive(
    snapshot_dir: &Path,
    archive_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let tmp_path = archive_path.with_extension("tmp");
    let bytes = build_snapshot_archive_bytes(snapshot_dir)?;
    fs::write(&tmp_path, bytes)?;
    fs::rename(tmp_path, archive_path)?;
    Ok(())
}

fn temporary_snapshot_path(snapshot_path: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let name = snapshot_path
        .file_name()
        .ok_or_else(|| format!("snapshot path has no final path segment: {}", snapshot_path.display()))?
        .to_string_lossy();
    Ok(snapshot_path.with_file_name(format!(".{name}.partial")))
}

fn build_snapshot_archive_bytes(snapshot_dir: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let root_name = snapshot_dir
        .file_name()
        .ok_or_else(|| format!("snapshot directory has no final path segment: {}", snapshot_dir.display()))?
        .to_string_lossy()
        .into_owned();

    let encoder = GzBuilder::new().mtime(0).write(Vec::new(), Compression::default());
    let mut tar = Builder::new(encoder);

    append_directory_entry(&mut tar, Path::new(&root_name))?;

    let mut entries = collect_directory_entries(snapshot_dir)?;
    entries.sort();

    for path in entries {
        let relative = path.strip_prefix(snapshot_dir)?;
        let archive_path = PathBuf::from(&root_name).join(relative);
        let metadata = fs::symlink_metadata(&path)?;

        if metadata.is_dir() {
            append_directory_entry(&mut tar, &archive_path)?;
        } else if metadata.is_file() {
            append_file_entry(&mut tar, &archive_path, &path)?;
        } else {
            return Err(format!("unsupported snapshot entry {}", path.display()).into());
        }
    }

    let encoder = tar.into_inner()?;
    Ok(encoder.finish()?)
}

fn collect_directory_entries(root: &Path) -> Result<Vec<PathBuf>, io::Error> {
    let mut entries = Vec::new();
    let mut pending = vec![root.to_path_buf()];

    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                pending.push(path.clone());
            }
            entries.push(path);
        }
    }

    Ok(entries)
}

fn copy_snapshot_contents(source: &Path, target: &Path) -> Result<(), io::Error> {
    let mut pending = vec![source.to_path_buf()];

    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            let relative = path
                .strip_prefix(source)
                .map_err(|err| io::Error::other(format!("invalid snapshot path prefix: {err}")))?;
            let file_type = entry.file_type()?;
            let target_path = if file_type.is_file() && relative == Path::new("tables") {
                target.join("tables").join("tvar")
            } else {
                target.join(relative)
            };

            if file_type.is_dir() {
                fs::create_dir_all(&target_path)?;
                pending.push(path);
            } else if file_type.is_file() {
                if let Some(parent) = target_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(&path, &target_path)?;
            } else {
                return Err(io::Error::other(format!("unsupported snapshot entry {}", path.display())));
            }
        }
    }

    Ok(())
}

fn append_directory_entry<W: io::Write>(tar: &mut Builder<W>, archive_path: &Path) -> io::Result<()> {
    let mut header = Header::new_gnu();
    header.set_entry_type(tar::EntryType::Directory);
    header.set_mode(0o755);
    header.set_size(0);
    header.set_mtime(0);
    header.set_uid(0);
    header.set_gid(0);
    header.set_cksum();
    tar.append_data(&mut header, archive_path, io::empty())
}

fn append_file_entry<W: io::Write>(tar: &mut Builder<W>, archive_path: &Path, file_path: &Path) -> io::Result<()> {
    let bytes = fs::read(file_path)?;
    let mut header = Header::new_gnu();
    header.set_entry_type(tar::EntryType::Regular);
    header.set_mode(0o644);
    header.set_size(bytes.len() as u64);
    header.set_mtime(0);
    header.set_uid(0);
    header.set_gid(0);
    header.set_cksum();
    tar.append_data(&mut header, archive_path, Cursor::new(bytes))
}
