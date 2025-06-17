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

use crate::metrics::track_system_metrics;
use amaru::stages::{bootstrap, Config, StorePath};
use amaru_kernel::{default_chain_dir, default_ledger_dir, network::NetworkName};
use clap::{ArgAction, Parser};
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
    #[arg(long, value_name = "NETWORK_ADDRESS", action = ArgAction::Append, required = true)]
    peer_address: Vec<String>,

    /// The target network to choose from.
    ///
    /// Should be one of 'mainnet', 'preprod', 'preview' or 'testnet:<magic>' where
    /// `magic` is a 32-bits unsigned value denoting a particular testnet.
    #[arg(
        long,
        value_name = "NETWORK",
        default_value_t = NetworkName::Preprod,
    )]
    network: NetworkName,

    /// Path of the ledger on-disk storage.
    #[arg(long, value_name = "DIR")]
    ledger_dir: Option<PathBuf>,

    /// Path of the chain on-disk storage.
    #[arg(long, value_name = "DIR")]
    chain_dir: Option<PathBuf>,

    /// The address to listen on for incoming connections.
    #[arg(long, value_name = "LISTEN_ADDRESS", default_value = super::DEFAULT_LISTEN_ADDRESS)]
    listen_address: String,

    /// The maximum number of downstream peers to connect to.
    #[arg(long, value_name = "MAX_DOWNSTREAM_PEERS", default_value_t = 10)]
    max_downstream_peers: usize,
}

pub async fn run(
    args: Args,
    metrics: Option<SdkMeterProvider>,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = parse_args(args)?;

    let metrics = metrics.map(track_system_metrics);

    let mut clients: Vec<(String, Arc<Mutex<PeerClient>>)> = vec![];
    for peer in &config.upstream_peers {
        let client =
            PeerClient::connect(peer.clone(), config.network.to_network_magic() as u64).await?;
        clients.push((peer.clone(), Arc::new(Mutex::new(client))));
    }

    let sync = bootstrap(config, clients)?;

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
    let network = args.network;
    let ledger_dir = args
        .ledger_dir
        .unwrap_or_else(|| default_ledger_dir(network).into());
    let chain_dir = args
        .chain_dir
        .unwrap_or_else(|| default_chain_dir(network).into());
    Ok(Config {
        ledger_store: StorePath::OnDisk(ledger_dir),
        chain_store: StorePath::OnDisk(chain_dir),
        upstream_peers: args.peer_address,
        network: args.network,
        network_magic: args.network.to_network_magic(),
        listen_address: args.listen_address,
        max_downstream_peers: args.max_downstream_peers,
    })
}
