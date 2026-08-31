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

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use amaru::{
    aws::{DEFAULT_BUCKET, DEFAULT_ENDPOINT, DEFAULT_PUBLIC_URL, DEFAULT_REGION, S3Config},
    bootstrap::bootstrap,
    default_chain_dir, default_ledger_dir, default_snapshots_dir,
    lifecycle::{Runnable, RuntimeKind},
};
use amaru_kernel::{Epoch, GlobalParameters, NetworkName, utils::path::relative_path};
use amaru_observability::{info, warn};
use clap::Parser;

#[derive(Debug, Parser)]
pub struct Args {
    /// Path of the chain on-disk storage.
    ///
    /// Defaults to ./chain.<NETWORK>.db when unspecified.
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::CHAIN_DIR,
    )]
    chain_dir: Option<PathBuf>,

    /// Path of the ledger on-disk storage.
    ///
    /// Defaults to ./ledger.<NETWORK>.db when unspecified.
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::LEDGER_DIR,
    )]
    ledger_dir: Option<PathBuf>,

    /// The target bootstrap epoch; this is the epoch Amaru will start from.
    ///
    /// At least 3 past epochs must exist. When omitted, this defaults the latest available epoch
    /// from known snapshots.
    #[arg(
        long = "epoch",
        value_name = amaru::value_names::UINT,
        env = amaru::env_vars::EPOCH,
    )]
    epoch: Option<Epoch>,

    /// Network to bootstrap the node for.
    #[arg(
        long,
        value_name = amaru::value_names::NETWORK,
        env = amaru::env_vars::NETWORK,
    )]
    network: NetworkName,

    /// Override network's global parameters for custom testnets.
    #[command(flatten)]
    global_parameters: GlobalParameters,

    /// Show global network parameter overrides, for custom testnets.
    #[arg(long)]
    pub(crate) help_global_parameters: bool,

    /// S3 bucket containing the bootstrap snapshots.
    ///
    /// Defaults to the official Amaru snapshot bucket.
    #[arg(
        long,
        value_name = amaru::value_names::BUCKET_NAME,
        env = "AMARU_S3_BUCKET",
        default_value = DEFAULT_BUCKET,
        help_heading = "S3 Snapshot Options",
    )]
    s3_bucket: String,

    /// S3-compatible endpoint URL.
    ///
    /// Defaults to the official Amaru R2 endpoint.
    #[arg(
        long,
        value_name = amaru::value_names::URL,
        env = "AMARU_S3_ENDPOINT",
        default_value = DEFAULT_ENDPOINT,
        help_heading = "S3 Snapshot Options",
    )]
    s3_endpoint: String,

    /// S3-compatible region.
    #[arg(
        long,
        value_name = amaru::value_names::S3_REGION,
        env = "AMARU_S3_REGION",
        default_value = DEFAULT_REGION,
        help_heading = "S3 Snapshot Options",
    )]
    s3_region: String,

    /// Public CDN base URL for anonymous snapshot downloads.
    ///
    /// Defaults to the official Amaru public R2 URL.
    #[arg(
        long,
        value_name = amaru::value_names::URL,
        env = "AMARU_S3_PUBLIC_URL",
        default_value = DEFAULT_PUBLIC_URL,
        help_heading = "S3 Snapshot Options",
    )]
    s3_public_url: String,
}

pub(crate) fn runnable(args: Args) -> Runnable {
    Runnable::exit_on_signal(RuntimeKind::Io, move || run(args))
}

async fn run(args: Args) -> anyhow::Result<()> {
    let network = args.network;

    let global_parameters = network.as_global_parameters().cloned().unwrap_or(args.global_parameters);

    let ledger_dir = args.ledger_dir.unwrap_or_else(|| default_ledger_dir(network).into());

    let chain_dir = args.chain_dir.unwrap_or_else(|| default_chain_dir(network).into());

    info!(
        cli::node::BOOTSTRAP,
        chain_dir = relative_path(&chain_dir)?.display().to_string(),
        ledger_dir = relative_path(&ledger_dir)?.display().to_string(),
        network,
        epoch = @args.epoch.map(|e| e.to_string()),
    );

    let ledger_dir_populated = is_populated(&ledger_dir)?;
    let chain_dir_populated = is_populated(&chain_dir)?;

    if ledger_dir_populated || chain_dir_populated {
        let mut messages = Vec::new();

        if ledger_dir_populated {
            let dir = relative_path(&ledger_dir)?.display().to_string();
            let hint = "ledger directory already exists: use another location or remove it manually";
            warn!(cli::ledger_db::EXIST, dir, hint);
            messages.push(format!("{hint} ({dir})"));
        }

        if chain_dir_populated {
            let dir = relative_path(&chain_dir)?.display().to_string();
            let hint = "chain directory already exists: use another location or remove it manually";
            warn!(cli::chain_db::EXIST, dir, hint);
            messages.push(format!("{hint} ({dir})"));
        }

        anyhow::bail!("{}", messages.join("; "));
    }

    bootstrap(
        network,
        &global_parameters,
        ledger_dir,
        chain_dir,
        default_snapshots_dir(network).into(),
        args.epoch,
        S3Config {
            bucket: args.s3_bucket,
            endpoint: args.s3_endpoint,
            region: args.s3_region,
            public_url: args.s3_public_url,
        },
    )
    .await
}

/// Whether `dir` holds anything. An empty directory is no more a database than a missing one, and
/// the build script keeps empty ledger directories around for cargo to watch.
///
/// A directory that cannot be read is not reported as empty: this guards existing databases, so it
/// must not let a bootstrap through on a failed inspection.
fn is_populated(dir: &Path) -> Result<bool, io::Error> {
    match fs::read_dir(dir) {
        Ok(mut entries) => Ok(entries.next().is_some()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(io::Error::new(err.kind(), format!("{}: {err}", dir.display()))),
    }
}
