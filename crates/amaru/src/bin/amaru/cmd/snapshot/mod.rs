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
    env, io,
    path::{Path, PathBuf},
};

use amaru::{default_snapshots_dir, lifecycle::Runnable};
use amaru_kernel::NetworkName;
use anyhow::Context;
use clap::Subcommand;

#[cfg(feature = "mithril")]
pub(crate) mod create;
pub(crate) mod epochs;
pub(crate) mod publish;
pub(crate) mod reindex;

#[derive(Debug, Subcommand)]
pub(crate) enum SnapshotCommand {
    /// List available bootstrap starting epochs, one per line in ascending order.
    ///
    /// Snapshots exist for the three preceding epochs of each listed epoch.
    /// Pass a listed epoch to node bootstrap --epoch.
    Epochs(epochs::Args),

    /// Create the three consecutive epoch snapshots needed for bootstrap.
    #[cfg(feature = "mithril")]
    Create(create::Args),

    /// Upload bootstrap snapshots to S3 and update the network index.
    Publish(publish::Args),

    /// Rebuild the snapshot index from archives stored in S3.
    Reindex(reindex::Args),
}

impl SnapshotCommand {
    pub(crate) fn into_runnable(self) -> Runnable {
        match self {
            Self::Epochs(args) => epochs::runnable(args),
            #[cfg(feature = "mithril")]
            Self::Create(args) => create::runnable(args),
            Self::Publish(args) => publish::runnable(args),
            Self::Reindex(args) => reindex::runnable(args),
        }
    }
}

fn executable_dir() -> anyhow::Result<PathBuf> {
    let executable = env::current_exe().with_context(|| "failed to determine the amaru executable path")?;

    executable.parent().map(Path::to_path_buf).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("amaru executable path has no parent directory: {}", executable.display()),
        )
        .into()
    })
}

fn default_snapshot_output_dir(network: NetworkName) -> anyhow::Result<PathBuf> {
    Ok(executable_dir()?.join(default_snapshots_dir(network)))
}
