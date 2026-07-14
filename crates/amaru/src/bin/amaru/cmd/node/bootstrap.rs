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

use std::{error::Error, fs::remove_dir_all, path::PathBuf};

use amaru::{
    aws::{DEFAULT_BUCKET, DEFAULT_ENDPOINT, DEFAULT_PUBLIC_URL, DEFAULT_REGION, S3Config},
    bootstrap::bootstrap,
    default_chain_dir, default_ledger_dir,
};
use amaru_kernel::{Epoch, GlobalParameters, NetworkName};
use clap::{ArgAction, Parser};
use tracing::{info, warn};

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

    /// Forcefully erase and overwrite the ledger database if it already exists.
    #[arg(
        long,
        action = ArgAction::SetTrue,
        default_value_t = false,
    )]
    force: bool,

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
    #[arg(long, env = "AMARU_S3_BUCKET", default_value = DEFAULT_BUCKET)]
    s3_bucket: String,

    /// S3-compatible endpoint URL (e.g. https://<id>.r2.cloudflarestorage.com).
    ///
    /// Defaults to the official Amaru R2 endpoint.
    #[arg(long, env = "AMARU_S3_ENDPOINT", default_value = DEFAULT_ENDPOINT)]
    s3_endpoint: String,

    /// S3 region (use "auto" for Cloudflare R2).
    #[arg(long, env = "AMARU_S3_REGION", default_value = DEFAULT_REGION)]
    s3_region: String,

    /// Public CDN base URL for anonymous snapshot downloads (e.g. https://pub-xxx.r2.dev).
    ///
    /// Defaults to the official Amaru public R2 URL.
    #[arg(long, env = "AMARU_S3_PUBLIC_URL", default_value = DEFAULT_PUBLIC_URL)]
    s3_public_url: String,
}

pub async fn run(args: Args) -> Result<(), Box<dyn Error>> {
    let network = args.network;

    let global_parameters = network.as_global_parameters().cloned().unwrap_or(args.global_parameters);

    let ledger_dir = args.ledger_dir.unwrap_or_else(|| default_ledger_dir(network).into());

    let chain_dir = args.chain_dir.unwrap_or_else(|| default_chain_dir(network).into());

    info!(
        _command = "node bootstrap",
        chain_dir = %chain_dir.to_string_lossy(),
        force = %args.force,
        ledger_dir = %ledger_dir.to_string_lossy(),
        network = %network,
        epoch = args.epoch
            .map(|e| Box::new(e.to_string()) as Box<dyn tracing::Value>)
            .unwrap_or_else(|| Box::new(tracing::field::Empty)),
        "running",
    );

    if ledger_dir.exists() || chain_dir.exists() {
        if !args.force {
            warn!(
                ledger_dir=%ledger_dir.to_string_lossy(),
                chain_dir=%chain_dir.to_string_lossy(),
                "ledger or chain directory already exists"
            );
            return Ok(());
        } else {
            if ledger_dir.exists() {
                info!(
                    ledger_dir=%ledger_dir.to_string_lossy(),
                    "forcing bootstrap, removing existing ledger directory"
                );
                remove_dir_all(&ledger_dir)?;
            }
            if chain_dir.exists() {
                info!(
                    chain_dir=%chain_dir.to_string_lossy(),
                    "forcing bootstrap, removing existing chain directory"
                );
                remove_dir_all(&chain_dir)?;
            }
        }
    }

    bootstrap(
        network,
        &global_parameters,
        ledger_dir,
        chain_dir,
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
