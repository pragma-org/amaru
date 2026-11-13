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
    io::{self, Write},
    path::Path,
};

use anyhow::Context;
use tempfile::NamedTempFile;

pub(super) fn prepare_key(
    write_raw: impl FnOnce(&mut NamedTempFile) -> io::Result<()>,
    path: &Path,
) -> anyhow::Result<NamedTempFile> {
    let mut file = NamedTempFile::new_in(parent(path))
        .with_context(|| format!("could not create temporary file for {}", path.display()))?;
    write_raw(&mut file).with_context(|| format!("could not write file for {}", path.display()))?;
    Ok(file)
}

fn parent(path: &Path) -> &Path {
    path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or(Path::new("."))
}

pub(super) fn sync_parent(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    fs::File::open(parent(path))?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Stage bytes beside their destination, with private permissions and flushed contents.
pub(super) fn prepare_file(path: &Path, bytes: &[u8]) -> anyhow::Result<NamedTempFile> {
    let file = prepare_key(|file| file.write_all(bytes), path)?;
    file.as_file().sync_all()?;
    Ok(file)
}

pub(super) fn ensure_absent(path: &Path) -> anyhow::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => anyhow::bail!("{} already exists", path.display()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("could not check {}", path.display())),
    }
}

/// Publish new files without replacing existing files, undoing partial publication on error.
/// Synchronize each destination parent before returning success. Retain published files if that fails.
pub(super) fn publish_new<'a>(files: impl IntoIterator<Item = (&'a Path, NamedTempFile)>) -> anyhow::Result<()> {
    let mut published: Vec<&Path> = Vec::new();
    for (path, file) in files {
        if let Err(error) = file.persist_noclobber(path) {
            for path in published {
                fs::remove_file(path)
                    .with_context(|| format!("could not remove partially created {}", path.display()))?;
            }
            return Err(error).with_context(|| format!("could not save {}", path.display()));
        }
        published.push(path);
    }
    for path in published {
        sync_parent(path).with_context(|| {
            format!("files published, but could not synchronize destination directory {}", parent(path).display())
        })?;
    }
    Ok(())
}
