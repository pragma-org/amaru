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

use std::{fs, io, path::Path};

use amaru::lifecycle::Runnable;
use anyhow::{Context, ensure};
use tempfile::NamedTempFile;

pub mod cert;
pub mod id;
pub mod kes;
pub mod vrf;

/// Stake pool operator tools.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum PoolCommand {
    #[command(subcommand)]
    Cert(cert::CertCommand),

    #[command(subcommand)]
    Kes(kes::KesCommand),

    #[command(subcommand)]
    Id(id::IdCommand),

    #[command(subcommand)]
    Vrf(vrf::VrfCommand),
}

impl PoolCommand {
    pub fn into_runnable(self) -> Runnable {
        match self {
            Self::Cert(cmd) => cmd.into_runnable(),
            Self::Kes(cmd) => cmd.into_runnable(),
            Self::Id(cmd) => cmd.into_runnable(),
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

    for path in [Some(signing_key), verification_key].iter().flatten() {
        let exists = path.try_exists().with_context(|| format!("could not check {}", path.display()))?;
        ensure!(!exists, "{} already exists", path.display());
    }

    let signing = prepare_key(write_signing, signing_key)?;

    if let Some(verification_key) = verification_key {
        let verification = prepare_key(write_verification, verification_key)?;
        verification
            .persist_noclobber(verification_key)
            .with_context(|| format!("could not save {}", verification_key.display()))?;
    };

    signing
        .persist_noclobber(signing_key)
        .inspect_err(|_| {
            if let Some(verification_key) = verification_key {
                let _ = fs::remove_file(verification_key);
            }
        })
        .with_context(|| format!("could not save {}", signing_key.display()))?;
    Ok(())
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
