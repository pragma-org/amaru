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

use amaru::lifecycle::Runnable;
use anyhow::{Context, ensure};
use tempfile::NamedTempFile;

pub mod cert;
pub mod cold;
pub mod id;
pub mod kes;
pub mod schedule;
pub mod vrf;

/// Stake pool operator tools.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum PoolCommand {
    #[command(subcommand)]
    Cert(cert::CertCommand),

    #[command(subcommand)]
    Cold(cold::ColdCommand),

    #[command(subcommand)]
    Kes(kes::KesCommand),

    #[command(subcommand)]
    Id(id::IdCommand),

    #[command(subcommand)]
    Schedule(schedule::ScheduleCommand),

    #[command(subcommand)]
    Vrf(vrf::VrfCommand),
}

impl PoolCommand {
    pub fn into_runnable(self) -> Runnable {
        match self {
            Self::Cert(cmd) => cmd.into_runnable(),
            Self::Cold(cmd) => cmd.into_runnable(),
            Self::Kes(cmd) => cmd.into_runnable(),
            Self::Id(cmd) => cmd.into_runnable(),
            Self::Schedule(cmd) => cmd.into_runnable(),
            Self::Vrf(cmd) => cmd.into_runnable(),
        }
    }
}

/// Write raw key files without replacing existing destinations.
fn write_key_pair(
    signing_key: &Path,
    verification_key: Option<&Path>,
    write_signing: impl FnOnce(&mut NamedTempFile) -> io::Result<()>,
    write_verification: impl FnOnce(&mut NamedTempFile) -> io::Result<()>,
) -> anyhow::Result<()> {
    if let Some(verification_key) = verification_key {
        ensure!(signing_key != verification_key, "key paths must differ");
    }

    [Some(signing_key), verification_key].into_iter().flatten().try_for_each(ensure_absent)?;
    let signing = prepare_key(write_signing, signing_key)?;
    let verification =
        verification_key.map(|path| prepare_key(write_verification, path).map(|file| (path, file))).transpose()?;
    publish_new(verification.into_iter().chain([(signing_key, signing)]))
}

fn prepare_key(
    write_raw: impl FnOnce(&mut NamedTempFile) -> io::Result<()>,
    path: &Path,
) -> anyhow::Result<NamedTempFile> {
    let mut file = NamedTempFile::new_in(parent(path))
        .with_context(|| format!("could not create temporary key file for {}", path.display()))?;
    write_raw(&mut file).with_context(|| format!("could not write key file for {}", path.display()))?;
    Ok(file)
}

fn parent(path: &Path) -> &Path {
    path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or(Path::new("."))
}

fn sync_parent(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    fs::File::open(parent(path))?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Stage bytes beside their destination, with private permissions and flushed contents.
fn prepare_file(path: &Path, bytes: &[u8]) -> anyhow::Result<NamedTempFile> {
    let file = prepare_key(|file| file.write_all(bytes), path)?;
    file.as_file().sync_all()?;
    Ok(file)
}

fn ensure_absent(path: &Path) -> anyhow::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => anyhow::bail!("{} already exists", path.display()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("could not check {}", path.display())),
    }
}

/// Publish new files without replacing existing files, undoing partial publication on error.
/// Synchronize each destination parent before returning success. Retain published files if that fails.
fn publish_new<'a>(files: impl IntoIterator<Item = (&'a Path, NamedTempFile)>) -> anyhow::Result<()> {
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
