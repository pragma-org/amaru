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
    DEFAULT_PEERS_LISTEN_ON, default_chain_dir, default_ledger_dir,
    lifecycle::{Runnable, RuntimeKind, ShutdownHandle},
    metrics::track_system_metrics,
    version,
};
use amaru_kernel::{ByteSize, EraHistory, GlobalParameters, NetworkName, PEER_SNAPSHOT_NETWORKS, utils::duration};
use amaru_mempool::MempoolConfig;
use amaru_metrics::Meter;
use amaru_node::{
    DEFAULT_PEERS_MAX_DOWNSTREAM, DEFAULT_PEERS_MAX_UPSTREAM,
    peer_snapshot::{embedded_configs_commit, load_embedded_peer_snapshot, load_peer_snapshot},
    stages::{
        build_node::build_and_run_node,
        config::{Config, LedgerConfig, MaxExtraLedgerSnapshots, StoreType},
    },
};
use amaru_observability::{error, info, info_record, info_span, warn};
use amaru_ouroboros::MempoolMsg;
use amaru_protocols::tx_submission::ResponderParams;
use amaru_pure_stage::{Sender, trace_buffer::TraceBuffer};
use amaru_stores::rocksdb::RocksDbConfig;
use amaru_tui as tui;
use anyhow::{Context, anyhow};
use clap::{self, ArgAction, Parser};
use parking_lot::Mutex;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

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
        env = amaru::env_vars::CHAIN_DB,
        display_order = 0,
        alias = "chain-dir",
    )]
    chain_db: Option<PathBuf>,

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
        env = amaru::env_vars::LEDGER_DB,
        display_order = 0,
        alias = "ledger-dir",
    )]
    ledger_db: Option<PathBuf>,

    /// The address to listen on for incoming connections.
    #[arg(
        long,
        value_name = amaru::value_names::ENDPOINT,
        env = amaru::env_vars::PEERS_LISTEN_ON,
        default_value = DEFAULT_PEERS_LISTEN_ON,
        display_order = 0,
        alias = "listen-address",
    )]
    peers_listen_on: String,

    /// Address for the HTTP transaction submit API.
    ///
    /// When set, starts an HTTP server exposing POST /api/submit/tx (Cardano Submit API).
    #[arg(
        long,
        value_name = amaru::value_names::ENDPOINT,
        env = amaru::env_vars::SUBMIT_API_LISTEN_ON,
        display_order = 0,
        alias = "submit-api-address"
    )]
    submit_api_listen_on: Option<String>,

    /// Disable the embedded terminal dashboard, even in an interactive terminal.
    #[arg(
        long,
        env = amaru::env_vars::NO_TUI,
        action = ArgAction::SetTrue,
        default_value_t = false,
        help_heading = "TUI",
    )]
    no_tui: bool,

    /// Maximum in-memory log retention for the TUI.
    ///
    /// Accepts a byte count or a size with a unit. SI units (`kB`, `MB`, `GB`) use powers
    /// of 1000; IEC units (`KiB`, `MiB`, `GiB`) use powers of 1024.
    /// The newest 70% keeps debug and up; the next 10% keeps info and up; then 10% warn
    /// and up; the oldest 10% keeps errors only.
    #[arg(
        long,
        env = amaru::env_vars::TUI_LOG_RETENTION,
        value_name = amaru::value_names::BYTE_SIZE,
        default_value = "100MiB",
        help_heading = "TUI",
    )]
    tui_log_retention: ByteSize,

    /// Upstream peer addresses to synchronize from.
    ///
    /// This option can be specified multiple times to connect to multiple peers.
    ///
    /// If not specified, defaults to the network-specific bootstrap peer.
    #[arg(
        long,
        value_name = amaru::value_names::ENDPOINT,
        env = amaru::env_vars::PEER,
        action = ArgAction::Append,
        value_delimiter = ',',
        num_args(0..),
        display_order = 0,
        alias = "peer-address",
    )]
    peer: Vec<String>,

    /// Path to a Cardano ledger peer snapshot JSON file (`bigLedgerPools`).
    ///
    /// Supplies stake-weighted big-ledger relays for peer selection at cold start,
    /// complementary to `--peer`. Compatible with cardano-node's
    /// `mainnet-peer-snapshot.json` (and similar per-network files).
    ///
    /// When omitted, Amaru uses the snapshot embedded at build time for known networks
    /// (for example mainnet, preprod, preview), if one was available when the binary was built.
    #[arg(
        long,
        value_name = amaru::value_names::FILEPATH,
        env = amaru::env_vars::PEERS_SNAPSHOT,
        display_order = 0,
        alias = "peer-snapshot"
    )]
    peers_snapshot: Option<PathBuf>,

    /// The maximum number of upstream peers to connect to.
    #[arg(
        long,
        value_name = amaru::value_names::UINT,
        env = amaru::env_vars::PEERS_MAX_UPSTREAM,
        default_value_t = DEFAULT_PEERS_MAX_UPSTREAM,
        display_order = 0,
        help_heading = "Advanced Options",
        alias = "upstream-peers",
    )]
    peers_max_upstream: usize,

    /// The maximum number of downstream peers allowed to connect.
    #[arg(
        long,
        value_name = amaru::value_names::UINT,
        env = amaru::env_vars::PEERS_MAX_DOWNSTREAM,
        default_value_t = DEFAULT_PEERS_MAX_DOWNSTREAM,
        display_order = 0,
        help_heading = "Advanced Options",
        alias = "downstream-peers",
    )]
    peers_max_downstream: usize,

    /// The maximum number of additional ledger snapshots to keep around.
    ///
    /// By default, Amaru only keeps the strict minimum of what's needed to operate.
    ///
    /// Should be a whole number >=0 or the string 'all' to keep all historical ledger snapshots
    /// (~2GB per epoch on Mainnet).
    #[arg(
        long,
        value_name = amaru::value_names::UINT_ALL,
        env = amaru::env_vars::LEDGER_MAX_EXTRA_SNAPSHOTS,
        default_value_t = MaxExtraLedgerSnapshots::default(),
        display_order = 0,
        help_heading = "Advanced Options",
        alias = "max-extra-ledger-snapshots",
    )]
    ledger_max_extra_snapshots: MaxExtraLedgerSnapshots,

    /// After removing a misbehaving upstream peer, wait this long before allowing it to be re-added.
    ///
    /// Provided as duration with units (e.g. 30s, 2min, ...)
    #[arg(
        long,
        value_name = amaru::value_names::DURATION,
        value_parser = duration::parse,
        env = amaru::env_vars::PEER_REMOVAL_COOLDOWN,
        default_value = "10min",
        display_order = 0,
        help_heading = "Advanced Options",
        alias = "peer-removal-cooldown-secs",
    )]
    peer_removal_cooldown: Duration,

    /// Using-slot mix formula (floors `!n`, weights `~n`, optional malus half-lives `@Nd`).
    ///
    /// Sources: `static`, `shared`, `snapshot`, `ledger`, and `inbound` (duplex inbound
    /// connections promoted to Using). Leaving a source out disables it; unused slots spill
    /// to remaining sources in declaration order.
    ///
    /// Example: `@12h, static!2, inbound~6, shared~6, snapshot~8, ledger~4@48h` (naked `@12h` is the default half-life for following sources)
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
        value_name = "MIN_ENTRIES,MAX_BYTE_SIZE",
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
        env = amaru::env_vars::TRACE_BUFFER_DUMP,
        display_order = 0,
        help_heading = "Advanced Options",
        alias = "dump-trace-buffer"
    )]
    trace_buffer_dump: Option<PathBuf>,

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
    pub fn peers_listen_on(&self) -> &str {
        &self.peers_listen_on
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
            usize::try_from(self.tui_log_retention).unwrap_or(usize::MAX),
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
            "chain_db" => Some(
                self.chain_db
                    .as_deref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| default_chain_dir(self.network)),
            ),
            "migrate_chain_db" => Some(self.migrate_chain_db.to_string()),
            "ledger_db" => Some(
                self.ledger_db
                    .as_deref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| default_ledger_dir(self.network)),
            ),
            "peers_listen_on" => Some(self.peers_listen_on.clone()),
            "submit_api_listen_on" => Some(self.submit_api_listen_on.clone().unwrap_or_else(|| "disabled".to_string())),
            "no_tui" => Some(self.no_tui.to_string()),
            "tui_log_retention" => Some(self.tui_log_retention.to_string()),
            "peer" => Some(self.peer.join(", ")),
            "peers_snapshot" => Some(peers_snapshot_value(self)),
            "peers_max_upstream" => Some(self.peers_max_upstream.to_string()),
            "peers_max_downstream" => Some(self.peers_max_downstream.to_string()),
            "ledger_max_extra_snapshots" => Some(self.ledger_max_extra_snapshots.to_string()),
            "peer_removal_cooldown" => Some(duration::format(&self.peer_removal_cooldown)),
            "peer_mix" => Some(self.peer_mix.clone()),
            "pid_file" => Some(
                self.pid_file
                    .as_deref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "disabled".to_string()),
            ),
            "trace_buffer" => Some(self.trace_buffer.clone().unwrap_or_else(|| "disabled".to_string())),
            "trace_buffer_dump" => Some(
                self.trace_buffer_dump
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

fn peers_snapshot_value(args: &Args) -> String {
    if let Some(path) = args.peers_snapshot.as_deref() {
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

async fn run(args: Args, meter: Meter, shutdown: ShutdownHandle) -> anyhow::Result<()> {
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
    shutdown.register_abort(running.abort_callback());
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

            let report = running.shutdown().await?;
            anyhow::ensure!(report.is_clean(), "node components failed during cleanup: {:?}", report.unexpected_exits);
            return Err(err);
        }
    };

    let term = running.termination();
    let exit_for_term = exit.clone();
    let consensus_died = Arc::new(AtomicBool::new(false));
    let consensus_died_flag = Arc::clone(&consensus_died);
    let termination_monitor = tokio::spawn(async move {
        term.await;
        if !exit_for_term.is_cancelled() {
            consensus_died_flag.store(true, Ordering::SeqCst);
            error!(setup::lifecycle::CONSENSUS_DIED);
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
            Ok(Err(err)) => {
                warn!(cli::node::SUBMIT_API_SHUTDOWN_FAILED, reason = "join_error", error = err.to_string());
            }
            Err(_) => {
                warn!(cli::node::SUBMIT_API_SHUTDOWN_FAILED, reason = "timeout");
            }
        }
    }

    if let Some(handle) = metrics {
        handle.abort();
    }

    let report = running.shutdown().await;
    termination_monitor.await?;
    let report = report?;
    anyhow::ensure!(report.is_clean(), "node components failed: {:?}", report.unexpected_exits);

    if consensus_died.load(Ordering::SeqCst) {
        anyhow::bail!("consensus stage graph terminated unexpectedly");
    }

    Ok(())
}

/// Start an HTTP API endpoint to allow local users to post CBOR-serialized transactions.
async fn start_submit_api(
    address: Option<std::net::SocketAddr>,
    mempool_sender: Sender<MempoolMsg>,
    exit: &CancellationToken,
) -> anyhow::Result<Option<tokio::task::JoinHandle<()>>> {
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
        Ok(()) => {
            info!(setup::trace_buffer::DUMPED, path = path.display().to_string());
        }
        Err(e) => {
            error!(setup::trace_buffer::DUMP_FAILED, path = path.display().to_string(), error = e.to_string());
        }
    }
}

fn parse_trace_buffer_limits(s: &str) -> anyhow::Result<(usize, usize)> {
    let parts: Vec<&str> = s.split(',').map(str::trim).filter(|p| !p.is_empty()).collect();
    if parts.len() != 2 {
        anyhow::bail!("expected two comma-separated integers (min_entries,max_size), got {s:?}");
    }
    let min_entries = parts[0].parse().with_context(|| format!("min_entries {:?}", parts[0]))?;
    let max_size = parts[1].parse().with_context(|| format!("max_size {:?}", parts[1]))?;
    Ok((min_entries, max_size))
}

#[allow(clippy::expect_used)]
fn parse_args(args: Args) -> anyhow::Result<Config> {
    let network = args.network;

    let era_history = match network.as_era_history().cloned() {
        Some(history) => history,
        None => {
            let path = args.era_history.as_deref().ok_or_else(|| anyhow!("missing era history for custom network"))?;
            EraHistory::load(path).with_context(|| format!("failed to load era history from {}", path.display()))?
        }
    };

    let global_parameters = network.as_global_parameters().cloned().unwrap_or(args.global_parameters);

    let ledger_db = args.ledger_db.unwrap_or_else(|| default_ledger_dir(network).into());
    if !std::fs::metadata(&ledger_db)
        .with_context(|| format!("failed to stat ledger_db `{}`", ledger_db.display()))?
        .is_dir()
    {
        anyhow::bail!(
            "ledger_db `{}` is not a directory, you need to run `amaru node bootstrap` first",
            ledger_db.display()
        );
    }

    let chain_db = args.chain_db.unwrap_or_else(|| default_chain_dir(network).into());
    if !std::fs::metadata(&chain_db)
        .with_context(|| format!("failed to stat chain_db `{}`", chain_db.display()))?
        .is_dir()
    {
        anyhow::bail!(
            "chain_db `{}` is not a directory, you need to run `amaru node bootstrap` first",
            chain_db.display()
        );
    }

    let network_magic = args.network.to_network_magic();
    let (peers_snapshot_peers, peers_snapshot_unresolved) = match args.peers_snapshot.as_deref() {
        Some(path) => {
            let snapshot = load_peer_snapshot(path, network_magic)?;
            log_loaded_snapshot(Some(path), &snapshot);
            (snapshot.peers, snapshot.unresolved)
        }
        None => match load_embedded_peer_snapshot(network)? {
            Some(snapshot) => {
                log_loaded_snapshot(None, &snapshot);
                (snapshot.peers, snapshot.unresolved)
            }
            None => {
                if PEER_SNAPSHOT_NETWORKS.contains(&network) {
                    warn!(setup::peer_snapshot::MISSING, network);
                }
                (BTreeSet::new(), BTreeSet::new())
            }
        },
    };

    let (trace_buffer_min_entries, trace_buffer_max_size) = match args.trace_buffer.as_deref() {
        None => (0usize, 0usize),
        Some(s) => parse_trace_buffer_limits(s)?,
    };

    let mempool = MempoolConfig::default();
    let tx_submission_params = ResponderParams::default();

    let era_history_path = args.era_history.map(|file| file.display().to_string());
    let global_parameters_json = matches!(network, NetworkName::Testnet(..))
        .then(|| serde_json::to_string(&global_parameters).expect("failed to serialise GlobalParameters to string?"));

    let _span = info_span!(
        cli::node::RUN,
        chain_db = chain_db.to_string_lossy(),
        ledger_db = ledger_db.to_string_lossy(),
        ledger_max_extra_snapshots = args.ledger_max_extra_snapshots.to_string(),
        mempool_max_bytes = &ByteSize::from_bytes(mempool.max_bytes).display_iec().to_string(),
        migrate_chain_db = args.migrate_chain_db,
        network = args.network,
        no_tui = args.no_tui,
        peer = args.peer.join(", "),
        peer_mix = &args.peer_mix,
        peer_removal_cooldown_ms = args.peer_removal_cooldown.as_millis() as u64,
        peers_listen_on = &args.peers_listen_on,
        peers_max_downstream = args.peers_max_downstream,
        peers_max_upstream = args.peers_max_upstream,
        peers_snapshot = args.peers_snapshot.as_deref().map(|p| p.display().to_string()).unwrap_or_else(|| {
            if peers_snapshot_peers.is_empty() && peers_snapshot_unresolved.is_empty() {
                "none".to_string()
            } else {
                format!("embedded{}", embedded_configs_commit().map(|sha| format!("@{sha}")).unwrap_or_default())
            }
        }),
        peers_snapshot_relays = peers_snapshot_peers.len() + peers_snapshot_unresolved.len(),
        pid_file = args.pid_file.clone().unwrap_or_default().display().to_string(),
        submit_api_listen_on = args.submit_api_listen_on.as_deref().unwrap_or("disabled"),
        trace_buffer = args.trace_buffer.as_deref().unwrap_or("disabled"),
        trace_buffer_dump = args
            .trace_buffer_dump
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "disabled".to_string()),
        tui_log_retention = args.tui_log_retention.to_string(),
        tx_submission_fetch_batch_bytes = tx_submission_params.fetch_batch_bytes.get(),
        tx_submission_inflight_timeout_ms =
            tx_submission_params.inflight_fetch_timeout.as_duration().as_millis() as u64,
        tx_submission_insert_timeout_ms = tx_submission_params.mempool_insert_timeout.as_duration().as_millis() as u64,
        tx_submission_max_window = tx_submission_params.max_window.get(),
    )
    .entered();
    if let Some(era_history) = era_history_path.as_deref() {
        info_record!(cli::node::RUN, era_history);
    }
    if let Some(global_parameters) = global_parameters_json.as_deref() {
        info_record!(cli::node::RUN, global_parameters);
    }

    Ok(Config {
        ledger_config: LedgerConfig {
            ledger_store: RocksDbConfig::new(ledger_db).with_shared_env(),
            network: args.network,
            global_parameters,
            era_history,
            max_extra_ledger_snapshots: args.ledger_max_extra_snapshots,
            emit_initial_stake_distribution_progress_ticks: !args.no_tui && std::io::stdout().is_terminal(),
            ..LedgerConfig::default()
        },
        chain_store: StoreType::RocksDb(RocksDbConfig::new(chain_db).with_shared_env()),
        upstream_peers: args.peer,
        peer_snapshot_peers: peers_snapshot_peers,
        peer_snapshot_unresolved: peers_snapshot_unresolved,
        target_upstream_peers: args.peers_max_upstream,
        target_downstream_peers: args.peers_max_downstream,
        network_magic: args.network.to_network_magic(),
        listen_address: args.peers_listen_on,
        migrate_chain_db: args.migrate_chain_db,
        submit_api_address: args.submit_api_listen_on,
        trace_buffer_min_entries,
        trace_buffer_max_size,
        trace_dump_path: args.trace_buffer_dump,
        peer_removal_cooldown: args.peer_removal_cooldown,
        peer_mix: args.peer_mix.parse().context("invalid --peer-mix")?,
        mempool,
        tx_submission_responder_params: tx_submission_params,
        ..Config::default()
    })
}

fn log_loaded_snapshot(path: Option<&Path>, snapshot: &amaru_node::peer_snapshot::PeerSnapshot) {
    let relays = snapshot.peers.len() + snapshot.unresolved.len();
    if relays == 0 {
        warn!(
            setup::peer_snapshot::EMPTY,
            path = path.map(|p| p.display().to_string()).unwrap_or_else(|| "embedded".into()),
            point = snapshot.point,
            pools = snapshot.pool_count
        );
    } else {
        info!(
            setup::peer_snapshot::LOADED,
            path = path.map(|p| p.display().to_string()).unwrap_or_else(|| "embedded".into()),
            point = snapshot.point,
            node_to_client_version = snapshot.node_to_client_version,
            pools = snapshot.pool_count,
            relays,
            configs_commit = embedded_configs_commit().unwrap_or("unknown")
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
                    setup::file_descriptors::TOO_LOW,
                    current_soft_fd_limit,
                    current_hard_fd_limit,
                    expected_min = EXPECTED_MIN_FOR_SOFT_FD_LIMIT,
                    hint = "Increase the limit for open files before starting Amaru (see ulimit -n)."
                );
                Err(PreFlightError::NotEnoughFileDescriptors(EXPECTED_MIN_FOR_SOFT_FD_LIMIT, current_soft_fd_limit))
            } else {
                Ok(())
            }
        }
        Err(_err) => {
            warn!(setup::file_descriptors::UNKNOWN, expected_min = EXPECTED_MIN_FOR_SOFT_FD_LIMIT);
            Ok(())
        }
    }
}

#[cfg(not(unix))]
fn pre_flight_checks() -> Result<(), PreFlightError> {
    Ok(())
}
