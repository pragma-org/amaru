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
    path::{Path, PathBuf},
};

use amaru::lifecycle::{Runnable, RuntimeKind};
use amaru_kernel::cardano::text_envelope;
use amaru_ouroboros::kes::SecretKey;
use anyhow::{Context, ensure};
use clap::{Args, Subcommand};
use tempfile::NamedTempFile;

#[derive(Debug, Subcommand)]
pub(crate) enum KeysCommand {
    /// Manage KES signing keys.
    #[command(subcommand)]
    Kes(KesCommand),
}

#[derive(Debug, Subcommand)]
pub(crate) enum KesCommand {
    /// Generate a cardano-cli compatible KES key pair.
    Create(CreateArgs),
}

#[derive(Debug, Args)]
pub(crate) struct CreateArgs {
    /// Destination for the KES signing key.
    #[arg(long)]
    signing_key_file: PathBuf,

    /// Destination for the KES verification key.
    #[arg(long)]
    verification_key_file: PathBuf,
}

impl KeysCommand {
    pub(crate) fn into_runnable(self) -> Runnable {
        match self {
            Self::Kes(KesCommand::Create(args)) => {
                Runnable::exit_on_signal(RuntimeKind::Io, move || async move { create(args) })
            }
        }
    }
}

fn create(args: CreateArgs) -> anyhow::Result<()> {
    let CreateArgs { signing_key_file, verification_key_file } = args;
    ensure!(signing_key_file != verification_key_file, "KES key paths must differ");
    ensure!(!signing_key_file.exists(), "{} already exists", signing_key_file.display());
    ensure!(!verification_key_file.exists(), "{} already exists", verification_key_file.display());

    let (secret, public) = SecretKey::generate().context("could not generate KES key")?;
    let mut signing = NamedTempFile::new_in(parent(&signing_key_file))?;
    let mut verification = NamedTempFile::new_in(parent(&verification_key_file))?;
    text_envelope::write(&secret, &mut signing)?;
    text_envelope::write(&public, &mut verification)?;

    verification.persist_noclobber(&verification_key_file)?;
    signing.persist_noclobber(&signing_key_file).inspect_err(|_| {
        let _ = fs::remove_file(&verification_key_file);
    })?;
    Ok(())
}

fn parent(path: &Path) -> &Path {
    path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or(Path::new("."))
}
