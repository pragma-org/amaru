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
    collections::BTreeSet,
    io::{IsTerminal, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use amaru::{
    DEFAULT_LISTEN_ADDRESS, default_chain_dir, default_ledger_dir, default_peer_for_network,
    lifecycle::{Runnable, RuntimeKind, ShutdownHandle},
    metrics::track_system_metrics,
    version,
};
use amaru_kernel::{EraHistory, GlobalParameters, NetworkName, PEER_SNAPSHOT_NETWORKS};
use amaru_mempool::MempoolConfig;
use amaru_metrics::Meter;
use amaru_node::{
    DEFAULT_DOWNSTREAM_PEERS, DEFAULT_PEER_REMOVAL_COOLDOWN_SECS, DEFAULT_UPSTREAM_PEERS,
    peer_snapshot::{embedded_configs_commit, load_embedded_peer_snapshot, load_peer_snapshot},
    stages::{
        build_node::build_and_run_node,
        config::{Config, LedgerConfig, MaxExtraLedgerSnapshots, StoreType},
    },
};
use amaru_ouroboros::MempoolMsg;
use amaru_protocols::tx_submission::ResponderParams;
use amaru_pure_stage::{Sender, trace_buffer::TraceBuffer};
use amaru_stores::rocksdb::RocksDbConfig;
use amaru_tui as tui;
use clap::{self, ArgAction, Parser};
use parking_lot::Mutex;
use thiserror::Error;
use tokio_util::sync::CancellationToken;
#[cfg(unix)]
use tracing::error;
use tracing::{info, warn};

use crate::pid::optional_pid_file;

#[derive(Debug, Parser)]
pub struct Args {
    /// The target network to run against.
    #[arg(
        long,
        value_name = amaru::value_names::NETWORK,
        env = amaru::env_vars::NETWORK,
        display_order = 0,
    )]
    network: NetworkName,

    /// Path of the chain on-disk storage.
    ///
    /// Defaults to ./chain.<NETWORK>.db when unspecified.
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::CHAIN_DIR,
        display_order = 0,
    )]
    chain_dir: Option<PathBuf>,

    /// Flag to automatically migrate the chain database if needed.
    ///
    /// By default, the migration is not performed automatically, checkout `amaru dev chain migrate` command.
    #[arg(
        long,
        env = amaru::env_vars::MIGRATE_CHAIN_DB,
        action = ArgAction::SetTrue,
        default_value_t = false,
        display_order = 0,
    )]
    migrate_chain_db: bool,

    /// Path of the ledger on-disk storage.
    ///
    /// Defaults to ./ledger.<NETWORK>.db when unspecified.
    #[arg(
        long,
        value_name = amaru::value_names::DIRECTORY,
        env = amaru::env_vars::LEDGER_DIR,
        display_order = 0,
    )]
    ledger_dir: Option<PathBuf>,

    /// The address to listen on for incoming connections.
    #[arg(
        long,
        value_name = amaru::value_names::ENDPOINT,
        env = amaru::env_vars::LISTEN_ADDRESS,
        default_value = DEFAULT_LISTEN_ADDRESS,
        display_order = 0,
    )]
    listen_address: String,

    /// Address for the HTTP transaction submit API.
    ///
    /// When set, starts an HTTP server exposing POST /api/submit/tx (Cardano Submit API).
    #[arg(
        long,
        value_name = amaru::value_names::ENDPOINT,
        env = amaru::env_vars::SUBMIT_API_ADDRESS,
        display_order = 0,
    )]
    submit_api_address: Option<String>,

    /// Disable the embedded terminal dashboard, even in an interactive terminal.
    #[arg(
        long,
        env = amaru::env_vars::NO_TUI,
        action = ArgAction::SetTrue,
        default_value_t = false,
        help_heading = "TUI",
    )]
    no_tui: bool,

    /// Upstream peer addresses to synchronize from.
    ///
    /// This option can be specified multiple times to connect to multiple peers.
    ///
    /// If not specified, defaults to the network-specific bootstrap peer.
    #[arg(
        long,
        value_name = amaru::value_names::ENDPOINT,
        env = amaru::env_vars::PEER_ADDRESS,
        action = ArgAction::Append,
        value_delimiter = ',',
        num_args(0..),
        display_order = 0,
    )]
    peer_address: Vec<String>,

    /// Path to a Cardano ledger peer snapshot JSON file (`bigLedgerPools`).
    ///
    /// Supplies stake-weighted big-ledger relays for peer selection at cold start,
    /// complementary to `--peer-address`. Compatible with cardano-node's
    /// `mainnet-peer-snapshot.json` (and similar per-network files).
    ///
    /// When omitted, Amaru uses the snapshot embedded at build time for known networks
    /// (for example mainnet, preprod, preview), if one was available when the binary was built.
    #[arg(
        long,
        value_name = amaru::value_names::FILEPATH,
        env = amaru::env_vars::PEER_SNAPSHOT,
        display_order = 0,
    )]
    peer_snapshot: Option<PathBuf>,

    /// The number of upstream peers to connect to.
    #[arg(
        long,
        value_name = amaru::value_names::UINT,
        env = amaru::env_vars::UPSTREAM_PEERS,
        default_value_t = DEFAULT_UPSTREAM_PEERS,
        display_order = 0,
        help_heading = "Advanced Options",
    )]
    upstream_peers: usize,

    /// The maximum number of downstream peers allowed to connect.
    #[arg(
        long,
        value_name = amaru::value_names::UINT,
        env = amaru::env_vars::DOWNSTREAM_PEERS,
        default_value_t = DEFAULT_DOWNSTREAM_PEERS,
        display_order = 0,
        help_heading = "Advanced Options",
    )]
    downstream_peers: usize,

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
        display_order = 0,
        help_heading = "Advanced Options",
    )]
    max_extra_ledger_snapshots: MaxExtraLedgerSnapshots,

    /// After removing a misbehaving upstream peer, wait this many seconds before allowing it to be re-added.
    #[arg(
        long,
        value_name = amaru::value_names::UINT,
        env = amaru::env_vars::PEER_REMOVAL_COOLDOWN_SECS,
        default_value_t = DEFAULT_PEER_REMOVAL_COOLDOWN_SECS,
        display_order = 0,
        help_heading = "Advanced Options",
    )]
    peer_removal_cooldown_secs: u64,

    /// Outbound peer source mix formula (floors `!n`, weights `~n`, optional malus half-lives `@Nd`).
    ///
    /// Leaving a source out of the formula disables it, peer slots not used by the formula are filled from the remaining sources in proportion to their weights.
    ///
    /// Example: `@12h, static!2, shared~6, snapshot~8, ledger~4@48h` (naked `@12h` is the default half-life for following sources)
    #[arg(
        long,
        value_name = amaru::value_names::PEER_MIX,
        env = amaru::env_vars::PEER_MIX,
        default_value = amaru_consensus::stages::peer_selection::DEFAULT_PEER_MIX,
        display_order = 0,
        help_heading = "Advanced Options",
    )]
    peer_mix: String,

    /// Path to the PID file managed by Amaru.
    #[arg(
        long,
        value_name = amaru::value_names::FILEPATH,
        env = amaru::env_vars::PID_FILE,
        display_order = 0,
        help_heading = "Advanced Options",
    )]
    pid_file: Option<PathBuf>,

    /// Stage graph trace buffer: `min_entries,max_total_bytes` (e.g. `100,1000000`).
    ///
    /// Omit or use `0,0` to disable recording (default).
    #[arg(
        long,
        value_name = "MIN_ENTRIES,MAX_SIZE",
        env = amaru::env_vars::TRACE_BUFFER,
        display_order = 0,
        help_heading = "Advanced Options",
    )]
    trace_buffer: Option<String>,

    /// Concatenate raw CBOR trace entries to this file when the node shuts down.
    ///
    /// This is useful in conjunction with the `--trace-buffer` flag to capture the trace of the stage graph.
    #[arg(
        long,
        value_name = amaru::value_names::FILEPATH,
        env = amaru::env_vars::DUMP_TRACE_BUFFER,
        display_order = 0,
        help_heading = "Advanced Options",
    )]
    dump_trace_buffer: Option<PathBuf>,

    /// Path to a JSON era history file overriding the network default.
    ///
    /// This is required for generated custom testnets whose epoch length or era bounds differ from
    /// Amaru's built-in network profiles.
    ///
    /// For an example, see <https://github.com/pragma-org/amaru/blob/main/crates/amaru-kernel/src/cardano/snapshots/amaru_kernel__cardano__era_history__tests__mainnet_era_history.snap>
    #[arg(
        long,
        value_name = amaru::value_names::FILEPATH,
        env = amaru::env_vars::ERA_HISTORY,
        display_order = 0,
        help_heading = "Network Global Parameters Overrides",
    )]
    era_history: Option<PathBuf>,

    /// Override network's global parameters for custom testnets.
    #[command(flatten)]
    global_parameters: GlobalParameters,

    /// Show global network parameter overrides, for custom testnets.
    #[arg(long)]
    pub(crate) help_global_parameters: bool,
}

impl Args {
    pub fn listen_address(&self) -> &str {
        &self.listen_address
    }

    pub fn tui_settings(&self) -> tui::Settings {
        let global_parameters = self.effective_global_parameters();

        tui::Settings::new(
            self.no_tui,
            tui::StartupContext::new(
                std::process::id(),
                self.network.to_string(),
                version::display_version(),
                format!("{}/{}", version::target_os(), version::target_arch()),
                MempoolConfig::default().max_bytes,
                &global_parameters,
                self.network.as_protocol_parameters(),
                self.network
                    .as_era_history()
                    .cloned()
                    .or_else(|| self.era_history.as_deref().and_then(|path| EraHistory::load(path).ok())),
                tui::ConfigSection::from_runtime_settings(self),
            ),
        )
    }

    fn effective_global_parameters(&self) -> GlobalParameters {
        self.network.as_global_parameters().cloned().unwrap_or_else(|| self.global_parameters.clone())
    }
}

impl tui::RuntimeSettingsSource for Args {
    fn value_for(&self, id: &str) -> Option<String> {
        let global_parameters = self.effective_global_parameters();

        match id {
            "network" => Some(self.network.to_string()),
            "chain_dir" => Some(
                self.chain_dir
                    .as_deref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| default_chain_dir(self.network)),
            ),
            "migrate_chain_db" => Some(self.migrate_chain_db.to_string()),
            "ledger_dir" => Some(
                self.ledger_dir
                    .as_deref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| default_ledger_dir(self.network)),
            ),
            "listen_address" => Some(self.listen_address.clone()),
            "submit_api_address" => Some(self.submit_api_address.clone().unwrap_or_else(|| "disabled".to_string())),
            "no_tui" => Some(self.no_tui.to_string()),
            "peer_address" => Some(peer_addresses_value(self)),
            "peer_snapshot" => Some(peer_snapshot_value(self)),
            "upstream_peers" => Some(self.upstream_peers.to_string()),
            "downstream_peers" => Some(self.downstream_peers.to_string()),
            "max_extra_ledger_snapshots" => Some(self.max_extra_ledger_snapshots.to_string()),
            "peer_removal_cooldown_secs" => Some(self.peer_removal_cooldown_secs.to_string()),
            "peer_mix" => Some(self.peer_mix.clone()),
            "pid_file" => Some(
                self.pid_file
                    .as_deref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "disabled".to_string()),
            ),
            "trace_buffer" => Some(self.trace_buffer.clone().unwrap_or_else(|| "disabled".to_string())),
            "dump_trace_buffer" => Some(
                self.dump_trace_buffer
                    .as_deref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "disabled".to_string()),
            ),
            "era_history" => Some(
                self.era_history
                    .as_deref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| self.network.to_string()),
            ),
            "consensus_security_param" => Some(global_parameters.consensus_security_param.to_string()),
            "epoch_length_scale_factor" => Some(global_parameters.epoch_length_scale_factor.to_string()),
            "active_slot_coeff_inverse" => Some(global_parameters.active_slot_coeff_inverse.to_string()),
            "max_lovelace_supply" => Some(global_parameters.max_lovelace_supply.to_string()),
            "slots_per_kes_period" => Some(global_parameters.slots_per_kes_period.to_string()),
            "max_kes_evolution" => Some(global_parameters.max_kes_evolution.to_string()),
            "system_start" => Some(global_parameters.system_start.to_string()),
            _ => None,
        }
    }
}

fn peer_addresses_value(args: &Args) -> String {
    if args.peer_address.is_empty() {
        default_peer_for_network(args.network).to_string()
    } else {
        args.peer_address.join(", ")
    }
}

fn peer_snapshot_value(args: &Args) -> String {
    if let Some(path) = args.peer_snapshot.as_deref() {
        return path.display().to_string();
    }

    if PEER_SNAPSHOT_NETWORKS.contains(&args.network) {
        return embedded_configs_commit()
            .map(|commit| format!("embedded ({commit})"))
            .unwrap_or_else(|| "none".to_string());
    }

    "none".to_string()
}

const SUBMIT_API_JOIN_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) fn runnable(args: Args) -> Runnable {
    Runnable::soft(RuntimeKind::Node, move |shutdown, meter| run(args, meter, shutdown))
}

async fn run(args: Args, meter: Meter, shutdown: ShutdownHandle) -> Result<(), Box<dyn std::error::Error>> {
    let _pid_file = optional_pid_file(args.pid_file.clone());

    let mut config = parse_args(args)?;
    let trace_dump_path = config.trace_dump_path.clone();
    let submit_api_address = config.submit_api_address()?;
    pre_flight_checks()?;

    let meter = Arc::new(meter);
    let metrics = track_system_metrics(meter.clone())?;
    config.meter = Some(meter);
    // Explicit handle: node stages must run on this process's Tokio runtime.
    let running = build_and_run_node(config, &tokio::runtime::Handle::current())?;

    // Main-thread signal path can abort stages without scheduling this future.
    let running_for_abort = running.clone();
    shutdown.register_abort(move || running_for_abort.request_abort());
    if shutdown.is_cancelled() {
        running.request_abort();
    }

    let exit = shutdown.token();
    let submit_api_handle = match start_submit_api(submit_api_address, running.mempool_sender(), &exit).await {
        Ok(handle) => handle,
        Err(err) => {
            let trace_buffer = running.trace_buffer().clone();
            running.request_abort();
            dump_trace_buffer_to_file(trace_dump_path.as_deref(), &trace_buffer);

            if let Some(handle) = metrics.as_ref() {
                handle.abort();
            }

            return Err(err);
        }
    };

    let term = running.termination();
    let exit_for_term = exit.clone();
    let consensus_died = Arc::new(AtomicBool::new(false));
    let consensus_died_flag = Arc::clone(&consensus_died);
    tokio::spawn(async move {
        term.await;
        if !exit_for_term.is_cancelled() {
            consensus_died_flag.store(true, Ordering::SeqCst);
            tracing::error!(
                "Consensus died, this should not happen! Please report this incl. preceding logs to the Amaru team."
            );
            exit_for_term.cancel();
        }
    });

    exit.cancelled().await;

    let trace_buffer = running.trace_buffer().clone();
    running.request_abort();
    dump_trace_buffer_to_file(trace_dump_path.as_deref(), &trace_buffer);

    if let Some(handle) = submit_api_handle {
        match tokio::time::timeout(SUBMIT_API_JOIN_TIMEOUT, handle).await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => warn!(error = %err, "submit API task ended with join error"),
            Err(_) => warn!("submit API did not shut down within timeout"),
        }
    }

    if let Some(handle) = metrics {
        handle.abort();
    }

    if consensus_died.load(Ordering::SeqCst) {
        return Err("consensus stage graph terminated unexpectedly".into());
    }

    Ok(())
}

/// Start an HTTP API endpoint to allow local users to post CBOR-serialized transactions.
async fn start_submit_api(
    address: Option<std::net::SocketAddr>,
    mempool_sender: Sender<MempoolMsg>,
    exit: &CancellationToken,
) -> Result<Option<tokio::task::JoinHandle<()>>, Box<dyn std::error::Error>> {
    let Some(addr) = address else {
        return Ok(None);
    };
    let shutdown = exit.child_token();
    let (handle, _) = amaru_node::submit_api::start(addr, mempool_sender, shutdown).await?;
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

#[allow(clippy::expect_used)]
fn parse_args(args: Args) -> Result<Config, Box<dyn std::error::Error>> {
    let network = args.network;

    let era_history = network.as_era_history().cloned().map(Ok).unwrap_or_else(|| {
        args.era_history
            .as_deref()
            .ok_or_else(|| "missing era history for custom network".into())
            .and_then(|path| EraHistory::load(path).map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) }))
    })?;

    let global_parameters = network.as_global_parameters().cloned().unwrap_or(args.global_parameters);

    let ledger_dir = args.ledger_dir.unwrap_or_else(|| default_ledger_dir(network).into());
    if !ledger_dir.is_dir() {
        return Err(format!(
            "ledger_dir `{}` is not a directory, you need to run `amaru node bootstrap` first",
            ledger_dir.display()
        )
        .into());
    }

    let chain_dir = args.chain_dir.unwrap_or_else(|| default_chain_dir(network).into());
    if !chain_dir.is_dir() {
        return Err(format!(
            "chain_dir `{}` is not a directory, you need to run `amaru node bootstrap` first",
            chain_dir.display()
        )
        .into());
    }

    // Use network-specific default peer if no peer-address was provided
    let peer_address = if args.peer_address.is_empty() {
        vec![default_peer_for_network(network).to_string()]
    } else {
        args.peer_address
    };

    let network_magic = args.network.to_network_magic();
    let peer_snapshot_peers = match args.peer_snapshot.as_deref() {
        Some(path) => {
            let snapshot = load_peer_snapshot(path, network_magic)?;
            log_loaded_snapshot(Some(path), &snapshot);
            snapshot.peers
        }
        None => match load_embedded_peer_snapshot(network)? {
            Some(snapshot) => {
                log_loaded_snapshot(None, &snapshot);
                snapshot.peers
            }
            None => {
                if PEER_SNAPSHOT_NETWORKS.contains(&network) {
                    warn!(
                        network = %network,
                        "no embedded peer snapshot for this network; continuing without snapshot peers"
                    );
                }
                BTreeSet::new()
            }
        },
    };

    let (trace_buffer_min_entries, trace_buffer_max_size) = match args.trace_buffer.as_deref() {
        None => (0usize, 0usize),
        Some(s) => parse_trace_buffer_limits(s)?,
    };

    let trace_dump_path = args.dump_trace_buffer;

    let mempool = MempoolConfig::default();
    let tx_submission_params = ResponderParams::default();

    info!(
        _command = "node run",
        chain_dir = %chain_dir.to_string_lossy(),
        ledger_dir = %ledger_dir.to_string_lossy(),
        listen_address = args.listen_address,
        max_extra_ledger_snapshots = %args.max_extra_ledger_snapshots,
        migrate_chain_db = args.migrate_chain_db,
        network = %args.network,
        era_history = args.era_history
            .map(|file| Box::new(file.display().to_string()) as Box<dyn tracing::Value>)
            .unwrap_or_else(|| Box::new(tracing::field::Empty)),
        global_parameters = if matches!(network, NetworkName::Testnet(..)) {
            Box::new(serde_json::to_string(&global_parameters).expect("failed to serialise GlobalParameters to string?")) as Box<dyn tracing::Value>
        } else {
            Box::new(tracing::field::Empty)
        },
        peer_address = %peer_address.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "),
        peer_snapshot = %args
            .peer_snapshot
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| {
                if peer_snapshot_peers.is_empty() {
                    "none".to_string()
                } else {
                    format!(
                        "embedded{}",
                        embedded_configs_commit()
                            .map(|sha| format!("@{sha}"))
                            .unwrap_or_default()
                    )
                }
            }),
        peer_snapshot_relays = peer_snapshot_peers.len(),
        pid_file = %args.pid_file.unwrap_or_default().to_string_lossy(),
        submit_api_address = %args.submit_api_address.as_deref().unwrap_or("disabled"),
        trace_buffer_min_entries,
        trace_buffer_max_size,
        trace_dump_path = %trace_dump_path.as_deref().map(|p| p.display().to_string()).unwrap_or_else(|| "disabled".to_string()),
        peer_removal_cooldown_secs = args.peer_removal_cooldown_secs,
        mempool_max_bytes = ?mempool.max_bytes,
        tx_submission_max_window = tx_submission_params.max_window.get(),
        tx_submission_fetch_batch_bytes = tx_submission_params.fetch_batch_bytes.get(),
        tx_submission_inflight_timeout_ms = tx_submission_params.inflight_fetch_timeout.as_duration().as_millis() as u64,
        tx_submission_insert_timeout_ms = tx_submission_params.mempool_insert_timeout.as_duration().as_millis() as u64,
        "running"
    );

    Ok(Config {
        ledger_config: LedgerConfig {
            ledger_store: RocksDbConfig::new(ledger_dir).with_shared_env(),
            network: args.network,
            global_parameters,
            era_history,
            max_extra_ledger_snapshots: args.max_extra_ledger_snapshots,
            emit_initial_stake_distribution_progress_ticks: !args.no_tui && std::io::stdout().is_terminal(),
            ..LedgerConfig::default()
        },
        chain_store: StoreType::RocksDb(RocksDbConfig::new(chain_dir).with_shared_env()),
        upstream_peers: peer_address,
        peer_snapshot_peers,
        target_upstream_peers: args.upstream_peers,
        target_downstream_peers: args.downstream_peers,
        network_magic: args.network.to_network_magic(),
        listen_address: args.listen_address,
        migrate_chain_db: args.migrate_chain_db,
        submit_api_address: args.submit_api_address,
        trace_buffer_min_entries,
        trace_buffer_max_size,
        trace_dump_path,
        peer_removal_cooldown_secs: args.peer_removal_cooldown_secs,
        peer_mix: args.peer_mix.parse().map_err(|e| anyhow::anyhow!("invalid --peer-mix: {e}"))?,
        mempool,
        tx_submission_responder_params: tx_submission_params,
        ..Config::default()
    })
}

fn log_loaded_snapshot(path: Option<&Path>, snapshot: &amaru_node::peer_snapshot::PeerSnapshot) {
    if snapshot.peers.is_empty() {
        warn!(
            path = %path.map(|p| p.display().to_string()).unwrap_or_else(|| "embedded".into()),
            point = %snapshot.point,
            pools = snapshot.pool_count,
            "peer snapshot loaded but contains no relays"
        );
    } else {
        info!(
            path = %path.map(|p| p.display().to_string()).unwrap_or_else(|| "embedded".into()),
            point = %snapshot.point,
            node_to_client_version = snapshot.node_to_client_version,
            pools = snapshot.pool_count,
            relays = snapshot.peers.len(),
            configs_commit = embedded_configs_commit().unwrap_or("unknown"),
            "loaded peer snapshot"
        );
    }
}

#[allow(dead_code, reason = "Debug instance is unused but useful to keep")]
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
