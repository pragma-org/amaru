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

use crate::pid::ProcessIdHandle;
use crate::{cmd::connect_to_peer, metrics::track_system_metrics};
use amaru::stages::{Config, MaxExtraLedgerSnapshots, StoreType, bootstrap};
use amaru_kernel::{default_chain_dir, default_ledger_dir, network::NetworkName};
use amaru_stores::rocksdb::RocksDbConfig;
use clap::{ArgAction, Parser};
use opentelemetry_sdk::metrics::SdkMeterProvider;
use pallas_network::facades::PeerClient;
use std::{path::PathBuf, time::Duration};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, trace, warn};

#[derive(Debug, Parser)]
pub struct Args {
    /// Upstream peer addresses to synchronize from.
    ///
    /// This option can be specified multiple times to connect to multiple peers.
    /// At least one peer address must be specified.
    #[arg(long, value_name = "NETWORK_ADDRESS", env = "AMARU_PEER_ADDRESS",
        action = ArgAction::Append, required = true, value_delimiter = ',')]
    peer_address: Vec<String>,

    /// The target network to choose from.
    ///
    /// Should be one of 'mainnet', 'preprod', 'preview' or 'testnet_<magic>' where
    /// `magic` is a 32-bits unsigned value denoting a particular testnet.
    #[arg(
        long,
        value_name = "NETWORK",
        env = "AMARU_NETWORK",
        default_value_t = NetworkName::Preprod,
    )]
    network: NetworkName,

    /// Path of the ledger on-disk storage.
    #[arg(long, value_name = "DIR", env("AMARU_LEDGER_DIR"))]
    ledger_dir: Option<PathBuf>,

    /// Path of the chain on-disk storage.
    #[arg(long, value_name = "DIR", env("AMARU_CHAIN_DIR"))]
    chain_dir: Option<PathBuf>,

    /// The address to listen on for incoming connections.
    #[arg(long, value_name = "LISTEN_ADDRESS", env = "AMARU_LISTEN_ADDRESS", default_value = super::DEFAULT_LISTEN_ADDRESS
    )]
    listen_address: String,

    /// The maximum number of downstream peers to connect to.
    #[arg(long, value_name = "MAX_DOWNSTREAM_PEERS", default_value_t = 10)]
    max_downstream_peers: usize,

    /// The maximum number of additional ledger snapshots to keep around. By default, Amaru only
    /// keeps the strict minimum of what's needed to operate.
    ///
    /// Should be a whole number >=0 or the string 'all' to keep all historical ledger snapshots
    /// (~2GB per epoch on Mainnet).
    #[arg(
        long,
        value_name = "MAX_EXTRA_LEDGER_SNAPSHOTS",
        env = "AMARU_MAX_EXTRA_LEDGER_SNAPSHOTS",
        default_value_t = MaxExtraLedgerSnapshots::default(),
    )]
    max_extra_ledger_snapshots: MaxExtraLedgerSnapshots,

    /// Path to the PID file, managed by Amaru.
    #[arg(long, value_name = "FILE", env = "AMARU_PID_FILE")]
    pid_file: Option<PathBuf>,
}

pub async fn run(
    args: Args,
    meter_provider: Option<SdkMeterProvider>,
) -> Result<(), Box<dyn std::error::Error>> {
    // This is being kept-alive so that the `drop` function
    // is called when the `ProcessIdHandler` goes out of scope.
    // That allows the file to be cleaned up before Amaru gracefully exits.
    // It is not vital that this cleanup happens, but it's nice to have :)
    let _pid_file = args.pid_file.as_ref().map(|path| {
        ProcessIdHandle::new(path)
            .inspect(|pid_file| {
                debug!(
                    "created PID File {}, current PID: {}",
                    pid_file,
                    pid_file.pid()
                )
            })
            .inspect_err(|e| warn!("failed to create or write to PID file: {} ", e))
    });

    let config = parse_args(args)?;
    pre_flight_checks()?;

    let metrics = meter_provider
        .clone()
        .map(track_system_metrics)
        .transpose()?;

    let mut clients: Vec<(String, PeerClient)> = vec![];
    for peer in &config.upstream_peers {
        let client = connect_to_peer(peer, &config.network).await?;
        clients.push((peer.clone(), client));
    }

    let exit = amaru::exit::hook_exit_token();

    let sync = bootstrap(config, clients, exit.clone(), meter_provider).await?;

    run_pipeline(gasket::daemon::Daemon::new(sync), exit).await;

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
        ledger_store: StoreType::RocksDb(RocksDbConfig::new(ledger_dir).with_shared_env()),
        chain_store: StoreType::RocksDb(RocksDbConfig::new(chain_dir).with_shared_env()),
        upstream_peers: args.peer_address,
        network: args.network,
        network_magic: args.network.to_network_magic(),
        listen_address: args.listen_address,
        max_downstream_peers: args.max_downstream_peers,
        max_extra_ledger_snapshots: args.max_extra_ledger_snapshots,
    })
}

#[derive(Debug, Error)]
pub enum PreFlightError {
    #[error("File descriptors limit too low: minimum required {0}, available {1}")]
    NotEnoughFileDescriptors(u64, u64),
}

#[cfg(unix)]
fn pre_flight_checks() -> Result<(), PreFlightError> {
    use rlimit::{Resource, getrlimit};
    /// We can follow mainnet with the following amount of FDs but could crash with less.
    /// RocksDB can consume some amount of FDs for its internal operations.
    /// System metrics collection with sysinfo also consumes FDs.
    /// And of course we still need some FDs for network connections and so on.
    const MIN_FD_LIMIT: u64 = 1_000;

    match getrlimit(Resource::NOFILE) {
        Ok((soft_fd_limit, hard_fd_limit)) => {
            if soft_fd_limit < MIN_FD_LIMIT {
                error!(
                    %soft_fd_limit,
                    %hard_fd_limit,
                    %MIN_FD_LIMIT,
                    "Increase the limit for open files before starting Amaru. See ulimit -n",
                );
                Err(PreFlightError::NotEnoughFileDescriptors(
                    MIN_FD_LIMIT,
                    soft_fd_limit,
                ))
            } else {
                Ok(())
            }
        }
        Err(_err) => {
            warn!(%MIN_FD_LIMIT, "Unable to query rlimit for max open files.");
            Ok(())
        }
    }
}

#[cfg(not(unix))]
fn pre_flight_checks() -> Result<(), PreFlightError> {
    Ok(())
}
