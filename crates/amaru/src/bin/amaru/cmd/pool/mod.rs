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

use std::{io, path::Path};

use amaru::lifecycle::Runnable;
use anyhow::ensure;
use tempfile::NamedTempFile;

use super::files::{ensure_absent, prepare_file, prepare_key, publish_new, sync_parent};

pub mod cert;
pub mod cold;
pub mod create;
pub mod id;
pub mod kes;
pub mod schedule;
pub mod vrf;

/// Stake pool operator tools.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum PoolCommand {
    Create(create::CreateArgs),

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
            Self::Create(args) => args.into_runnable(),
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
