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
    #[command(flatten)]
    network: amaru::args::Network,

    #[command(flatten, next_help_heading = "Storage options")]
    db_chain: amaru::args::DbChain,

    /// Flag to automatically migrate the chain database if needed.
    ///
    /// By default, the migration is not performed automatically, checkout `amaru dev chain migrate` command.
    #[arg(
        long,
        env = amaru::env_vars::DB_CHAIN_AUTOMATIC_MIGRATION,
        action = ArgAction::SetTrue,
        default_value_t = false,
        display_order = 0,
        help_heading = "Storage options",
        alias = "migrate-chain-db",
    )]
    db_chain_automatic_migration: bool,

    #[command(flatten, next_help_heading = "Storage options")]
    db_ledger: amaru::args::DbLedger,

    /// The maximum number of additional ledger snapshots to keep around.
    ///
    /// By default, Amaru only keeps the strict minimum of what's needed to operate.
    ///
    /// Should be a whole number >=0 or the string 'all' to keep all historical ledger snapshots
    /// (~2GB per epoch on Mainnet).
    #[arg(
        long,
        value_name = amaru::value_names::UINT_ALL,
        env = amaru::env_vars::DB_LEDGER_MAX_EXTRA_SNAPSHOTS,
        default_value_t = MaxExtraLedgerSnapshots::default(),
        display_order = 0,
        help_heading = "Storage options",
        alias = "max-extra-ledger-snapshots",
    )]
    db_ledger_max_extra_snapshots: MaxExtraLedgerSnapshots,

    /// KES signing key, as an unencrypted cardano-cli `kes.skey` text envelope.
    ///
    /// Together with `--operator-vrf` and `--operator-operational-certificate`, the node forges
    /// blocks. Preprod, preview, and other testnets only. Mainnet refuses these flags.
    #[arg(
        long,
        value_name = amaru::value_names::FILEPATH,
        env = amaru::env_vars::OPERATOR_KES,
        display_order = 0,
        help_heading = "Block forging options",
        alias = "kes-signing-key-file"
    )]
    operator_kes: Option<PathBuf>,

    /// Operational certificate, as an unencrypted cardano-cli `node.cert` text envelope.
    ///
    /// The file includes the cold verification key. No separate cold-key file is read.
    #[arg(
        long,
        value_name = amaru::value_names::FILEPATH,
        env = amaru::env_vars::OPERATOR_OPERATIONAL_CERTIFICATE,
        display_order = 0,
        help_heading = "Block forging options",
        alias = "operational-certificate",
    )]
    operator_operational_certificate: Option<PathBuf>,

    /// VRF signing key, as an unencrypted cardano-cli `vrf.skey` text envelope.
    #[arg(
        long,
        value_name = amaru::value_names::FILEPATH,
        env = amaru::env_vars::OPERATOR_VRF,
        display_order = 0,
        help_heading = "Block forging options",
        alias = "vrf-signing-key-file",
    )]
    operator_vrf: Option<PathBuf>,

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
        help_heading = "Peers options",
        alias = "peer-address",
    )]
    peer: Vec<String>,

    /// The address to listen on for incoming connections.
    #[arg(
        long,
        value_name = amaru::value_names::ENDPOINT,
        env = amaru::env_vars::PEERS_LISTEN_ON,
        default_value = DEFAULT_PEERS_LISTEN_ON,
        display_order = 0,
        help_heading = "Peers options",
        alias = "listen-address",
    )]
    peers_listen_on: String,

    /// The maximum number of downstream peers allowed to connect.
    #[arg(
        long,
        value_name = amaru::value_names::UINT,
        env = amaru::env_vars::PEERS_MAX_DOWNSTREAM,
        default_value_t = DEFAULT_PEERS_MAX_DOWNSTREAM,
        display_order = 0,
        help_heading = "Peers options",
        alias = "downstream-peers",
    )]
    peers_max_downstream: usize,

    /// The maximum number of upstream peers to connect to.
    #[arg(
        long,
        value_name = amaru::value_names::UINT,
        env = amaru::env_vars::PEERS_MAX_UPSTREAM,
        default_value_t = DEFAULT_PEERS_MAX_UPSTREAM,
        display_order = 0,
        help_heading = "Peers options",
        alias = "upstream-peers",
    )]
    peers_max_upstream: usize,

    /// After removing a misbehaving upstream peer, wait this long before allowing it to be re-added.
    ///
    /// Provided as duration with units (e.g. 30s, 2min, ...)
    #[arg(
        long,
        value_name = amaru::value_names::DURATION,
        value_parser = duration::parse,
        env = amaru::env_vars::PEERS_REMOVAL_COOLDOWN,
        default_value = "10min",
        display_order = 0,
        help_heading = "Peers options",
        alias = "peer-removal-cooldown-secs",
    )]
    peers_removal_cooldown: Duration,

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
        help_heading = "Peers options",
        alias = "peer-snapshot"
    )]
    peers_snapshot: Option<PathBuf>,

    /// Using-slot mix formula (floors `!n`, weights `~n`, (optional) half-lives `@Nd`).
    ///
    /// Sources: `static`, `shared`, `snapshot`, `ledger`, and `inbound` (duplex inbound
    /// connections promoted to Using). Leaving a source out disables it; unused slots spill
    /// to remaining sources in declaration order.
    ///
    /// Example: `@12h, static!2, inbound~6, shared~6, snapshot~8, ledger~4@48h` (naked `@12h` is the default half-life for following sources)
    #[arg(
        long,
        value_name = amaru::value_names::PEERS_MIX,
        env = amaru::env_vars::PEERS_MIX,
        default_value = amaru_consensus::stages::peer_selection::DEFAULT_PEERS_MIX,
        display_order = 0,
        help_heading = "Peers options",
        alias = "peer-mix"
    )]
    peers_mix: String,

    /// Disable the embedded terminal dashboard, even in an interactive terminal.
    #[arg(
        long,
        env = amaru::env_vars::TUI_OFF,
        action = ArgAction::SetTrue,
        default_value_t = false,
        help_heading = "TUI options",
        display_order = 0,
        alias = "no-tui",
    )]
    tui_off: bool,

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
        help_heading = "TUI options",
        display_order = 0,
    )]
    tui_log_retention: ByteSize,

    /// Address for the HTTP transaction submit API.
    ///
    /// When set, starts an HTTP server exposing POST /api/submit/tx (Cardano Submit API).
    #[arg(
        long,
        value_name = amaru::value_names::ENDPOINT,
        env = amaru::env_vars::SUBMIT_API_LISTEN_ON,
        help_heading = "Advanced options",
        display_order = 0,
        alias = "submit-api-address"
    )]
    submit_api_listen_on: Option<String>,

    /// Path to the PID file managed by Amaru.
    #[arg(
        long,
        value_name = amaru::value_names::FILEPATH,
        env = amaru::env_vars::PID_EXPORT,
        help_heading = "Advanced options",
        display_order = 0,
        alias = "pid-file"
    )]
    pid_export: Option<PathBuf>,

    /// Stage graph trace buffer: `min_entries,max_total_bytes` (e.g. `100,1000000`).
    ///
    /// Omit or use `0,0` to disable recording (default).
    #[arg(
        long,
        value_name = amaru::value_names::TRACE_BUFFER,
        env = amaru::env_vars::TRACE_BUFFER,
        help_heading = "Advanced options",
        display_order = 0,
    )]
    trace_buffer: Option<String>,

    /// Concatenate raw CBOR trace entries to this file when the node shuts down.
    ///
    /// This is useful in conjunction with the `--trace-buffer` flag to capture the trace of the stage graph.
    #[arg(
        long,
        value_name = amaru::value_names::FILEPATH,
        env = amaru::env_vars::TRACE_BUFFER_DUMP,
        help_heading = "Advanced options",
        display_order = 0,
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
        help_heading = "Network global parameters overrides (for custom devnets)",
        display_order = 0,
    )]
    era_history: Option<PathBuf>,

    /// Override network's global parameters for custom testnets / devnets.
    ///
    /// DO NOT override for known networks (e.g. mainnet, preprod, preview, ...), as these parameters are set in stone.
    #[command(flatten, next_help_heading = "Network global parameters")]
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
        let network = NetworkName::from(self.network);

        tui::Settings::new(
            self.tui_off,
            tui::StartupContext::new(
                std::process::id(),
                network.to_string(),
                version::display_version(),
                format!("{}/{}", version::target_os(), version::target_arch()),
                MempoolConfig::default().max_bytes,
                &global_parameters,
                network.as_protocol_parameters(),
                network
                    .as_era_history()
                    .cloned()
                    .or_else(|| self.era_history.as_deref().and_then(|path| EraHistory::load(path).ok())),
                tui::ConfigSection::from_runtime_settings(self),
            ),
            usize::try_from(self.tui_log_retention).unwrap_or(usize::MAX),
        )
    }

    fn effective_global_parameters(&self) -> GlobalParameters {
        NetworkName::from(self.network)
            .as_global_parameters()
            .cloned()
            .unwrap_or_else(|| self.global_parameters.clone())
    }
}

impl tui::RuntimeSettingsSource for Args {
    fn value_for(&self, id: &str) -> Option<String> {
        let global_parameters = self.effective_global_parameters();
        let network = NetworkName::from(self.network);

        match id {
            "network" => Some(network.to_string()),
            "db_chain" => Some(
                self.db_chain
                    .path()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| default_chain_dir(network)),
            ),
            "db_chain_automatic_migration" => Some(self.db_chain_automatic_migration.to_string()),
            "db_ledger" => Some(
                self.db_ledger
                    .path()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| default_ledger_dir(network)),
            ),
            "db_ledger_max_extra_snapshots" => Some(self.db_ledger_max_extra_snapshots.to_string()),
            "submit_api_listen_on" => Some(self.submit_api_listen_on.clone().unwrap_or_else(|| "disabled".to_string())),
            "tui_off" => Some(self.tui_off.to_string()),
            "tui_log_retention" => Some(self.tui_log_retention.to_string()),
            "peer" => Some(self.peer.join(", ")),
            "peers_listen_on" => Some(self.peers_listen_on.clone()),
            "peers_max_downstream" => Some(self.peers_max_downstream.to_string()),
            "peers_max_upstream" => Some(self.peers_max_upstream.to_string()),
            "peers_mix" => Some(self.peers_mix.clone()),
            "peers_removal_cooldown" => Some(duration::format(&self.peers_removal_cooldown)),
            "peers_snapshot" => Some(peers_snapshot_value(self)),
            "pid_export" => Some(
                self.pid_export
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
                    .unwrap_or_else(|| network.to_string()),
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

    if PEER_SNAPSHOT_NETWORKS.contains(&NetworkName::from(args.network)) {
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
    let _pid = optional_pid_file(args.pid_export.clone());

    let mut config = parse_args(args)?;
    let trace_dump_path = config.trace_dump_path.clone();
    let submit_api_address = config.submit_api_address()?;
    pre_flight_checks()?;

    let meter = Arc::new(meter);
    let metrics = track_system_metrics(meter.clone())?;
    config.meter = Some(meter);
    // Explicit handle: node stages must run on this process's Tokio runtime.
    let running = build_and_run_node(config, &tokio::runtime::Handle::current()).await?;

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
    let network = NetworkName::from(args.network);

    let era_history = match network.as_era_history().cloned() {
        Some(history) => history,
        None => {
            let path = args.era_history.as_deref().ok_or_else(|| anyhow!("missing era history for custom network"))?;
            EraHistory::load(path).with_context(|| format!("failed to load era history from {}", path.display()))?
        }
    };

    let global_parameters = network.as_global_parameters().cloned().unwrap_or(args.global_parameters);

    let forging_credentials = amaru_ouroboros::forging_credentials_from_files(
        network,
        u64::from(global_parameters.max_kes_evolution),
        args.operator_kes.as_deref(),
        args.operator_vrf.as_deref(),
        args.operator_operational_certificate.as_deref(),
    )?;

    let db_ledger = args.db_ledger.into_path_buf(network);
    if !std::fs::metadata(&db_ledger)
        .with_context(|| format!("failed to stat db_ledger `{}`", db_ledger.display()))?
        .is_dir()
    {
        anyhow::bail!(
            "db_ledger `{}` is not a directory, you need to run `amaru node bootstrap` first",
            db_ledger.display()
        );
    }

    let db_chain = args.db_chain.into_path_buf(network);
    if !std::fs::metadata(&db_chain)
        .with_context(|| format!("failed to stat db_chain `{}`", db_chain.display()))?
        .is_dir()
    {
        anyhow::bail!(
            "db_chain `{}` is not a directory, you need to run `amaru node bootstrap` first",
            db_chain.display()
        );
    }

    let network_magic = network.to_network_magic();
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
        db_chain = db_chain.to_string_lossy(),
        db_chain_automatic_migration = args.db_chain_automatic_migration,
        db_ledger = db_ledger.to_string_lossy(),
        db_ledger_max_extra_snapshots = args.db_ledger_max_extra_snapshots.to_string(),
        mempool_max_bytes = &ByteSize::from_bytes(mempool.max_bytes).display_iec().to_string(),
        network,
        peer = args.peer.join(", "),
        peers_mix = &args.peers_mix,
        peers_removal_cooldown_ms = args.peers_removal_cooldown.as_millis() as u64,
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
        pid_export = args.pid_export.clone().unwrap_or_default().display().to_string(),
        submit_api_listen_on = args.submit_api_listen_on.as_deref().unwrap_or("disabled"),
        trace_buffer = args.trace_buffer.as_deref().unwrap_or("disabled"),
        trace_buffer_dump = args
            .trace_buffer_dump
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "disabled".to_string()),
        tui_log_retention = args.tui_log_retention.to_string(),
        tui_off = args.tui_off,
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
            ledger_store: RocksDbConfig::new(db_ledger).with_shared_env(),
            network,
            global_parameters,
            era_history,
            max_extra_ledger_snapshots: args.db_ledger_max_extra_snapshots,
            emit_initial_stake_distribution_progress_ticks: !args.tui_off && std::io::stdout().is_terminal(),
            ..LedgerConfig::default()
        },
        chain_store: StoreType::RocksDb(RocksDbConfig::new(db_chain).with_shared_env()),
        upstream_peers: args.peer,
        peer_snapshot_peers: peers_snapshot_peers,
        peer_snapshot_unresolved: peers_snapshot_unresolved,
        target_upstream_peers: args.peers_max_upstream,
        target_downstream_peers: args.peers_max_downstream,
        network_magic: network.to_network_magic(),
        listen_address: args.peers_listen_on,
        migrate_chain_db: args.db_chain_automatic_migration,
        submit_api_address: args.submit_api_listen_on,
        trace_buffer_min_entries,
        trace_buffer_max_size,
        trace_dump_path: args.trace_buffer_dump,
        peer_removal_cooldown: args.peers_removal_cooldown,
        peer_mix: args.peers_mix.parse().context("invalid --peers-mix")?,
        mempool,
        tx_submission_responder_params: tx_submission_params,
        forging_credentials: forging_credentials
            .map(|credentials| Arc::new(credentials) as Arc<dyn amaru_ouroboros::ForgingCredentials>),
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
