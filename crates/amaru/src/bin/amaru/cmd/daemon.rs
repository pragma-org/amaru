// Copyright 2024 PRAGMA
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

use crate::{config::NetworkName, metrics::track_system_metrics};
use amaru::sync::Config;
use amaru_consensus::consensus::nonce;
use clap::{builder::TypedValueParser as _, Parser};
use miette::IntoDiagnostic;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use pallas_network::facades::PeerClient;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error};

#[derive(Debug, Parser)]
pub struct Args {
    /// Upstream peer address to synchronize from.
    #[arg(long)]
    peer_address: String,

    /// The target network to choose from.
    #[arg(
        long,
        default_value_t = NetworkName::Mainnet,
        value_parser = clap::builder::PossibleValuesParser::new(NetworkName::possible_values())
            .map(|s| s.parse::<NetworkName>().unwrap()),
    )]
    network: NetworkName,

    /// Path of the ledger on-disk storage.
    #[arg(long, default_value = super::DEFAULT_LEDGER_DB_DIR)]
    ledger_dir: PathBuf,

    /// Path of the chain on-disk storage.
    #[arg(long, default_value = super::DEFAULT_CHAIN_DATABASE_PATH)]
    chain_database_path: PathBuf,

    /// Path to the directory containing blockchain data such as epoch nonces.
    #[arg(long, default_value = super::DEFAULT_DATA_DIR)]
    data_dir: PathBuf,
}

pub async fn run(args: Args, metrics: Option<SdkMeterProvider>) -> miette::Result<()> {
    let config = parse_args(args)?;

    let metrics = metrics.map(track_system_metrics);

    let client = Arc::new(Mutex::new(
        PeerClient::connect(config.upstream_peer.clone(), config.network_magic as u64)
            .await
            .into_diagnostic()?,
    ));

    let sync = amaru::sync::bootstrap(config, &client)?;

    let exit = crate::exit::hook_exit_token();

    run_pipeline(gasket::daemon::Daemon::new(sync), exit.clone()).await;

    if let Some(handle) = metrics {
        handle.abort();
    }

    Ok(())
}

pub async fn run_pipeline(pipeline: gasket::daemon::Daemon, exit: CancellationToken) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(5000)) => {
                if pipeline.should_stop() {
                    break;
                }
            }
            _ = exit.cancelled() => {
                debug!("exit requested");
                break;
            }
        }
    }

    debug!("shutting down pipeline");

    pipeline.teardown();
}

fn parse_args(args: Args) -> miette::Result<Config> {
    // TODO: Figure out from ledger + consensus store
    let root = args.data_dir.join(args.network.to_string());

    let nonces = read_csv(&root.join("nonces.csv"), nonce::from_csv)?;

    Ok(Config {
        ledger_dir: args.ledger_dir,
        chain_database_path: args.chain_database_path,
        upstream_peer: args.peer_address,
        network_magic: args.network.to_network_magic(),
        nonces,
    })
}

fn read_csv<F, T>(filepath: &PathBuf, with: F) -> miette::Result<T>
where
    F: FnOnce(&str) -> T,
{
    Ok(with(
        std::str::from_utf8(
            std::fs::read(filepath)
                .inspect_err(|e| {
                    error!(
                        "failed to read csv data file at {}: {e}",
                        filepath.as_path().to_str().unwrap_or_default(),
                    );
                })
                .into_diagnostic()?
                .as_slice(),
        )
        .into_diagnostic()?,
    ))
}
