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

use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

use amaru::{
    DEFAULT_LISTEN_ADDRESS, DEFAULT_NETWORK, DEFAULT_PEER_ADDRESS, default_chain_dir, default_ledger_dir,
    metrics::track_system_metrics,
    stages::{
        build_node::build_and_run_node,
        config::{Config, MaxExtraLedgerSnapshots, StoreType},
    },
};
use amaru_kernel::NetworkName;
use amaru_ouroboros::MempoolMsg;
use amaru_stores::rocksdb::RocksDbConfig;
use clap::{ArgAction, Parser};
use opentelemetry_sdk::metrics::SdkMeterProvider;
use parking_lot::Mutex;
use pure_stage::{Sender, trace_buffer::TraceBuffer};
use thiserror::Error;
use tracing::{error, info, warn};

use crate::pid::with_optional_pid_file;

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

    /// The address to listen on for incoming connections.
    #[arg(
        long,
        value_name = amaru::value_names::ENDPOINT,
        env = amaru::env_vars::LISTEN_ADDRESS,
        default_value = DEFAULT_LISTEN_ADDRESS,
    )]
    listen_address: String,

    /// The maximum number of downstream peers to connect to.
    #[arg(
        long,
        value_name = amaru::value_names::UINT,
        env = amaru::env_vars::MAX_DOWNSTREAM_PEERS,
        default_value_t = 10
    )]
    max_downstream_peers: usize,

    /// The maximum number of additional ledger snapshots to keep around.
    ///
    /// By default, Amaru only keeps the strict minimum of what's needed to operate.
    ///
    /// Should be a whole number >=0 or the string 'all' to keep all historical ledger snapshots
    /// (~2GB per epoch on Mainnet).
    #[arg(
        long,
        value_name = amaru::value_names::UINT_ALL,
        env = amaru::env_vars::MAX_EXTRA_LEDGER_SNAPSHOTS,
        default_value_t = MaxExtraLedgerSnapshots::default(),
    )]
    max_extra_ledger_snapshots: MaxExtraLedgerSnapshots,

    /// Flag to automatically migrate the chain database if needed.
    ///
    /// By default, the migration is not performed automatically, checkout `migrate-chain-db` command.
    #[arg(
        long,
        env = amaru::env_vars::MIGRATE_CHAIN_DB,
        action = ArgAction::SetTrue,
        default_value_t = false,
    )]
    migrate_chain_db: bool,

    /// The target network to run against.
    #[arg(
        long,
        value_name = amaru::value_names::NETWORK,
        env = amaru::env_vars::NETWORK,
        default_value_t = DEFAULT_NETWORK,
    )]
    network: NetworkName,

    /// Address for the HTTP transaction submit API.
    ///
    /// When set, starts an HTTP server exposing POST /api/submit/tx (Cardano Submit API).
    #[arg(
        long,
        value_name = amaru::value_names::ENDPOINT,
        env = amaru::env_vars::SUBMIT_API_ADDRESS,
    )]
    submit_api_address: Option<String>,

    /// Upstream peer addresses to synchronize from.
    ///
    /// This option can be specified multiple times to connect to multiple peers.
    ///
    /// At least one peer address must be specified.
    #[arg(
        long,
        value_name = amaru::value_names::ENDPOINT,
        env = amaru::env_vars::PEER_ADDRESS,
        default_value = DEFAULT_PEER_ADDRESS,
        action = ArgAction::Append,
        value_delimiter = ',',
        num_args(0..),
    )]
    peer_address: Vec<String>,

    /// After removing a misbehaving upstream peer, wait this many seconds before allowing it to be re-added.
    #[arg(
        long,
        value_name = amaru::value_names::UINT,
        env = amaru::env_vars::PEER_REMOVAL_COOLDOWN_SECS,
        default_value_t = amaru::DEFAULT_PEER_REMOVAL_COOLDOWN_SECS,
    )]
    peer_removal_cooldown_secs: u64,

    /// Path to the PID file managed by Amaru.
    #[arg(
        long,
        value_name = amaru::value_names::FILEPATH,
        env = amaru::env_vars::PID_FILE,
    )]
    pid_file: Option<PathBuf>,

    /// Stage graph trace buffer: `min_entries,max_total_bytes` (e.g. `100,1000000`).
    ///
    /// Omit or use `0,0` to disable recording (default).
    #[arg(
        long,
        value_name = "MIN_ENTRIES,MAX_SIZE",
        env = amaru::env_vars::TRACE_BUFFER,
    )]
    trace_buffer: Option<String>,

    /// Concatenate raw CBOR trace entries to this file when the node shuts down.
    ///
    /// This is useful in conjunction with the `--trace-buffer` flag to capture the trace of the stage graph.
    #[arg(
        long,
        value_name = amaru::value_names::FILEPATH,
        env = amaru::env_vars::DUMP_TRACE_BUFFER,
    )]
    dump_trace_buffer: Option<PathBuf>,
}

impl Args {
    pub fn listen_address(&self) -> &str {
        &self.listen_address
    }
}

pub async fn run(args: Args, meter_provider: Option<SdkMeterProvider>) -> Result<(), Box<dyn std::error::Error>> {
    with_optional_pid_file(args.pid_file.clone(), async |_pid_file| {
        let config = parse_args(args)?;
        let trace_dump_path = config.trace_dump_path.clone();
        let submit_api_address = config.submit_api_address()?;
        pre_flight_checks()?;

        let metrics = meter_provider.clone().map(track_system_metrics).transpose()?;
        let running = build_and_run_node(config, meter_provider)?;

        let exit = amaru::exit::hook_exit_token();
        let submit_api_handle = match start_submit_api(submit_api_address, running.mempool_sender(), &exit).await {
            Ok(handle) => handle,
            Err(err) => {
                let trace_buffer = running.trace_buffer().clone();
                running.abort();
                dump_trace_buffer_to_file(trace_dump_path.as_deref(), &trace_buffer);

                if let Some(handle) = metrics.as_ref() {
                    handle.abort();
                }

                return Err(err);
            }
        };

        let term = running.termination();
        let exit2 = exit.clone();
        tokio::spawn(async move {
            term.await;
            if !exit2.is_cancelled() {
                tracing::error!(
                    "Consensus died, this should not happen! Please report this incl. preceding logs to the Amaru team."
                );
                exit2.cancel();
            }
        });

        let trace_buffer = running.trace_buffer().clone();
        exit.cancelled().await;
        running.abort();
        dump_trace_buffer_to_file(trace_dump_path.as_deref(), &trace_buffer);

        if let Some(handle) = submit_api_handle {
            let _ = handle.await; // Let graceful shutdown complete
        }

        if let Some(handle) = metrics {
            handle.abort();
        }

        Ok(())
    })
    .await
}

/// Start an HTTP API endpoint to allow local users to post CBOR-serialized transactions.
async fn start_submit_api(
    address: Option<std::net::SocketAddr>,
    mempool_sender: Sender<MempoolMsg>,
    exit: &tokio_util::sync::CancellationToken,
) -> Result<Option<tokio::task::JoinHandle<()>>, Box<dyn std::error::Error>> {
    let Some(addr) = address else {
        return Ok(None);
    };
    let shutdown = exit.child_token();
    let (handle, _) = amaru::submit_api::start(addr, mempool_sender, shutdown).await?;
    Ok(Some(handle))
}

fn dump_trace_buffer_to_file(path: Option<&Path>, trace_buffer: &Arc<Mutex<TraceBuffer>>) {
    let Some(path) = path else {
        return;
    };
    let result = (|| -> std::io::Result<()> {
        let mut file = std::fs::File::create(path)?;
        let guard = trace_buffer.lock();
        for chunk in guard.iter() {
            file.write_all(chunk)?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => tracing::info!(path = %path.display(), "wrote stage trace buffer dump"),
        Err(e) => tracing::error!(path = %path.display(), error = %e, "failed to write stage trace buffer dump"),
    }
}

fn parse_trace_buffer_limits(s: &str) -> Result<(usize, usize), String> {
    let parts: Vec<&str> = s.split(',').map(str::trim).filter(|p| !p.is_empty()).collect();
    if parts.len() != 2 {
        return Err(format!("expected two comma-separated integers (min_entries,max_size), got {s:?}"));
    }
    let min_entries = parts[0].parse().map_err(|e| format!("min_entries {:?}: {e}", parts[0]))?;
    let max_size = parts[1].parse().map_err(|e| format!("max_size {:?}: {e}", parts[1]))?;
    Ok((min_entries, max_size))
}

fn parse_args(args: Args) -> Result<Config, Box<dyn std::error::Error>> {
    let network = args.network;

    let ledger_dir = args.ledger_dir.unwrap_or_else(|| default_ledger_dir(network).into());

    let chain_dir = args.chain_dir.unwrap_or_else(|| default_chain_dir(network).into());

    let (trace_buffer_min_entries, trace_buffer_max_size) = match args.trace_buffer.as_deref() {
        None => (0usize, 0usize),
        Some(s) => parse_trace_buffer_limits(s)?,
    };

    let trace_dump_path = args.dump_trace_buffer;

    info!(
        _command = "run",
        chain_dir = %chain_dir.to_string_lossy(),
        ledger_dir = %ledger_dir.to_string_lossy(),
        listen_address = args.listen_address,
        max_downstream_peers = args.max_downstream_peers,
        max_extra_ledger_snapshots = %args.max_extra_ledger_snapshots,
        migrate_chain_db = args.migrate_chain_db,
        network = %args.network,
        peer_address = %args.peer_address.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "),
        pid_file = %args.pid_file.unwrap_or_default().to_string_lossy(),
        submit_api_address = %args.submit_api_address.as_deref().unwrap_or("disabled"),
        trace_buffer_min_entries,
        trace_buffer_max_size,
        trace_dump_path = %trace_dump_path.as_deref().map(|p| p.display().to_string()).unwrap_or_else(|| "disabled".to_string()),
        peer_removal_cooldown_secs = args.peer_removal_cooldown_secs,
        "running"
    );

    Ok(Config {
        ledger_store: StoreType::RocksDb(RocksDbConfig::new(ledger_dir).with_shared_env()),
        chain_store: StoreType::RocksDb(RocksDbConfig::new(chain_dir).with_shared_env()),
        upstream_peers: args.peer_address,
        network: args.network,
        network_magic: args.network.to_network_magic(),
        listen_address: args.listen_address,
        max_downstream_peers: args.max_downstream_peers,
        max_extra_ledger_snapshots: args.max_extra_ledger_snapshots,
        migrate_chain_db: args.migrate_chain_db,
        submit_api_address: args.submit_api_address,
        trace_buffer_min_entries,
        trace_buffer_max_size,
        trace_dump_path,
        peer_removal_cooldown_secs: args.peer_removal_cooldown_secs,
        ..Config::default()
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
    const EXPECTED_MIN_FOR_SOFT_FD_LIMIT: u64 = 1_000;

    match getrlimit(Resource::NOFILE) {
        Ok((current_soft_fd_limit, current_hard_fd_limit)) => {
            if current_soft_fd_limit < EXPECTED_MIN_FOR_SOFT_FD_LIMIT {
                error!(
                    %current_soft_fd_limit,
                    %current_hard_fd_limit,
                    %EXPECTED_MIN_FOR_SOFT_FD_LIMIT,
                    "Increase the limit for open files before starting Amaru (see ulimit -n).",
                );
                Err(PreFlightError::NotEnoughFileDescriptors(EXPECTED_MIN_FOR_SOFT_FD_LIMIT, current_soft_fd_limit))
            } else {
                Ok(())
            }
        }
        Err(_err) => {
            warn!(%EXPECTED_MIN_FOR_SOFT_FD_LIMIT, "Unable to query rlimit for max open files.");
            Ok(())
        }
    }
}

#[cfg(not(unix))]
fn pre_flight_checks() -> Result<(), PreFlightError> {
    Ok(())
}
