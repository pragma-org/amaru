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
use clap::{builder::TypedValueParser as _, ArgAction, Parser};
use opentelemetry_sdk::metrics::SdkMeterProvider;
use pallas_network::facades::PeerClient;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::trace;

#[derive(Debug, Parser)]
pub struct Args {
    /// Upstream peer addresses to synchronize from.
    ///
    /// This option can be specified multiple times to connect to multiple peers.
    /// At least one peer address must be specified.
    #[arg(long, action = ArgAction::Append, required = true)]
    peer_address: Vec<String>,

    /// The target network to choose from.
    #[arg(
        long,
        default_value_t = NetworkName::Preprod,
        value_parser = clap::builder::PossibleValuesParser::new(NetworkName::possible_values())
            .map(|s| s.parse::<NetworkName>().unwrap()),
    )]
    network: NetworkName,

    /// Path of the ledger on-disk storage.
    #[arg(long, default_value = super::DEFAULT_LEDGER_DB_DIR)]
    ledger_dir: PathBuf,

    /// Path of the chain on-disk storage.
    #[arg(long, default_value = super::DEFAULT_CHAIN_DATABASE_PATH)]
    chain_dir: PathBuf,

    /// Path to the directory containing blockchain data such as epoch nonces.
    #[arg(long, default_value = super::DEFAULT_DATA_DIR)]
    data_dir: PathBuf,
}

pub async fn run(
    args: Args,
    metrics: Option<SdkMeterProvider>,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = parse_args(args)?;

    let metrics = metrics.map(track_system_metrics);

    let mut clients: Vec<(String, Arc<Mutex<PeerClient>>)> = vec![];
    for peer in &config.upstream_peers {
        let client = PeerClient::connect(peer.clone(), config.network_magic as u64).await?;
        clients.push((peer.clone(), Arc::new(Mutex::new(client))));
    }

    let sync = amaru::sync::bootstrap(config, clients)?;

    let exit = amaru::exit::hook_exit_token();

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
                trace!("exit requested");
                break;
            }
        }
    }

    trace!("shutting down pipeline");

    pipeline.teardown();
}

fn parse_args(args: Args) -> Result<Config, Box<dyn std::error::Error>> {
    Ok(Config {
        ledger_dir: args.ledger_dir,
        chain_dir: args.chain_dir,
        upstream_peers: args.peer_address,
        network_magic: args.network.to_network_magic(),
    })
}
