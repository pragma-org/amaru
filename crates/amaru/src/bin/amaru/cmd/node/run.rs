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
    io::Write,
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
use amaru_kernel::{EraHistory, GlobalParameters, NetworkName, PEER_SNAPSHOT_NETWORKS, protocol_version};
use amaru_mempool::MempoolConfig;
use amaru_metrics::METRICS_METER_NAME;
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
use amaru_tui::{
    Config as TuiConfig, ConfigEntry as TuiConfigEntry, ConfigSection as TuiConfigSection, ProcessInfo,
    StartupContext as TuiStartupContext,
};
use anyhow::anyhow;
use clap::{self, ArgAction, Parser};
use opentelemetry::metrics::MeterProvider;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use parking_lot::Mutex;
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

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

    /// Comma-separated rolling windows used by the embedded terminal dashboard.
    #[arg(
        long,
        env = amaru::env_vars::TUI_WINDOWS,
        value_name = "DURATION[,DURATION...]",
        help_heading = "TUI",
    )]
    tui_windows: Option<String>,

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

    pub fn no_tui(&self) -> bool {
        self.no_tui
    }

    pub fn tui_windows(&self) -> Option<&str> {
        self.tui_windows.as_deref()
    }

    pub fn tui_startup_context(&self) -> TuiStartupContext {
        let protocol_parameters = self.network.as_protocol_parameters();
        let protocol_version = protocol_parameters
            .map(|parameters| protocol_version::fmt(&parameters.protocol_version))
            .unwrap_or_else(|| "unknown".to_string());

        let trusted_peers = self.trusted_peers();
        let global_parameters = self.effective_global_parameters();

        TuiStartupContext {
            process: ProcessInfo {
                network: self.network.to_string(),
                software_version: version::display_version().to_string(),
                target: format!("{}/{}", version::target_os(), version::target_arch()),
            },
            protocol_version,
            epoch_length: global_parameters.epoch_length(),
            active_slot_coeff_inverse: global_parameters.active_slot_coeff_inverse,
            max_lovelace_supply: global_parameters.max_lovelace_supply,
            system_start_millis: global_parameters.system_start,
            trusted_peers,
            runtime_sections: self.runtime_config_sections(),
            global_sections: self.global_config_sections(&global_parameters),
            protocol_sections: self.protocol_config_sections(protocol_parameters),
        }
    }

    fn effective_global_parameters(&self) -> GlobalParameters {
        self.network.as_global_parameters().cloned().unwrap_or_else(|| self.global_parameters.clone())
    }

    fn trusted_peers(&self) -> BTreeSet<String> {
        if self.peer_address.is_empty() {
            BTreeSet::from([default_peer_for_network(self.network).to_string()])
        } else {
            self.peer_address.iter().cloned().collect()
        }
    }

    fn runtime_config_sections(&self) -> Vec<TuiConfigSection> {
        vec![
            TuiConfigSection::new(
                "Storage and Chain",
                vec![
                    config_entry(
                        "chain dir",
                        "--chain-dir",
                        amaru::env_vars::CHAIN_DIR,
                        self.chain_dir.as_deref().map(path_value).unwrap_or_else(|| default_chain_dir(self.network)),
                    ),
                    config_entry(
                        "ledger dir",
                        "--ledger-dir",
                        amaru::env_vars::LEDGER_DIR,
                        self.ledger_dir.as_deref().map(path_value).unwrap_or_else(|| default_ledger_dir(self.network)),
                    ),
                    config_entry(
                        "migrate chain db",
                        "--migrate-chain-db",
                        amaru::env_vars::MIGRATE_CHAIN_DB,
                        bool_value(self.migrate_chain_db),
                    ),
                    config_entry(
                        "extra ledger snapshots",
                        "--max-extra-ledger-snapshots",
                        amaru::env_vars::MAX_EXTRA_LEDGER_SNAPSHOTS,
                        self.max_extra_ledger_snapshots.to_string(),
                    ),
                    config_entry(
                        "era history",
                        "--era-history",
                        amaru::env_vars::ERA_HISTORY,
                        self.era_history.as_deref().map(path_value).unwrap_or_else(|| era_history_value(self.network)),
                    ),
                ],
            ),
            TuiConfigSection::new(
                "Networking",
                vec![
                    config_entry("network", "--network", amaru::env_vars::NETWORK, self.network.to_string()),
                    config_entry(
                        "listen address",
                        "--listen-address",
                        amaru::env_vars::LISTEN_ADDRESS,
                        self.listen_address.clone(),
                    ),
                    config_entry(
                        "submit api",
                        "--submit-api-address",
                        amaru::env_vars::SUBMIT_API_ADDRESS,
                        self.submit_api_address.clone().unwrap_or_else(|| "disabled".to_string()),
                    ),
                    config_entry(
                        "peer address",
                        "--peer-address",
                        amaru::env_vars::PEER_ADDRESS,
                        peer_addresses_value(self),
                    ),
                    config_entry(
                        "peer snapshot",
                        "--peer-snapshot",
                        amaru::env_vars::PEER_SNAPSHOT,
                        peer_snapshot_value(self),
                    ),
                    config_entry(
                        "upstream peers",
                        "--upstream-peers",
                        amaru::env_vars::UPSTREAM_PEERS,
                        self.upstream_peers.to_string(),
                    ),
                    config_entry(
                        "downstream peers",
                        "--downstream-peers",
                        amaru::env_vars::DOWNSTREAM_PEERS,
                        self.downstream_peers.to_string(),
                    ),
                    config_entry(
                        "peer cooldown",
                        "--peer-removal-cooldown-secs",
                        amaru::env_vars::PEER_REMOVAL_COOLDOWN_SECS,
                        format!("{}s", self.peer_removal_cooldown_secs),
                    ),
                ],
            ),
            TuiConfigSection::new(
                "Process and Observability",
                vec![
                    config_entry("no tui", "--no-tui", amaru::env_vars::NO_TUI, bool_value(self.no_tui)),
                    config_entry(
                        "tui windows",
                        "--tui-windows",
                        amaru::env_vars::TUI_WINDOWS,
                        self.tui_windows.clone().unwrap_or_else(default_tui_windows_value),
                    ),
                    config_entry(
                        "pid file",
                        "--pid-file",
                        amaru::env_vars::PID_FILE,
                        self.pid_file.as_deref().map(path_value).unwrap_or_else(|| "disabled".to_string()),
                    ),
                    config_entry(
                        "trace buffer",
                        "--trace-buffer",
                        amaru::env_vars::TRACE_BUFFER,
                        self.trace_buffer.clone().unwrap_or_else(|| "disabled".to_string()),
                    ),
                    config_entry(
                        "dump trace buffer",
                        "--dump-trace-buffer",
                        amaru::env_vars::DUMP_TRACE_BUFFER,
                        self.dump_trace_buffer.as_deref().map(path_value).unwrap_or_else(|| "disabled".to_string()),
                    ),
                ],
            ),
        ]
    }

    fn global_config_sections(&self, global_parameters: &GlobalParameters) -> Vec<TuiConfigSection> {
        vec![TuiConfigSection::new(
            "Global Parameters",
            vec![
                config_entry(
                    "security param k",
                    "--consensus-security-param",
                    "AMARU_GLOBAL_CONSENSUS_SECURITY_PARAM",
                    global_parameters.consensus_security_param.to_string(),
                ),
                config_entry(
                    "epoch length factor",
                    "--epoch-length-scale-factor",
                    "AMARU_GLOBAL_EPOCH_LENGTH_SCALE_FACTOR",
                    global_parameters.epoch_length_scale_factor.to_string(),
                ),
                config_entry(
                    "active slot coeff inverse",
                    "--active-slot-coeff-inverse",
                    "AMARU_GLOBAL_ACTIVE_SLOT_COEFF_INVERSE",
                    global_parameters.active_slot_coeff_inverse.to_string(),
                ),
                config_entry(
                    "max lovelace supply",
                    "--max-lovelace-supply",
                    "AMARU_GLOBAL_MAX_LOVELACE_SUPPLY",
                    global_parameters.max_lovelace_supply.to_string(),
                ),
                config_entry(
                    "slots per KES period",
                    "--slots-per-kes-period",
                    "AMARU_GLOBAL_SLOTS_PER_KES_PERIOD",
                    global_parameters.slots_per_kes_period.to_string(),
                ),
                config_entry(
                    "max KES evolution",
                    "--max-kes-evolution",
                    "AMARU_GLOBAL_MAX_KES_EVOLUTION",
                    global_parameters.max_kes_evolution.to_string(),
                ),
                config_entry(
                    "system start",
                    "--system-start",
                    "AMARU_GLOBAL_SYSTEM_START",
                    global_parameters.system_start.to_string(),
                ),
            ],
        )]
    }

    fn protocol_config_sections(
        &self,
        protocol_parameters: Option<&amaru_kernel::ProtocolParameters>,
    ) -> Vec<TuiConfigSection> {
        let Some(protocol_parameters) = protocol_parameters else {
            return Vec::default();
        };

        vec![
            TuiConfigSection::new(
                "Protocol Parameters · Network",
                vec![
                    label_entry("max block body size", protocol_parameters.max_block_body_size.to_string()),
                    label_entry("max transaction size", protocol_parameters.max_transaction_size.to_string()),
                    label_entry("max block header size", protocol_parameters.max_block_header_size.to_string()),
                    label_entry("max tx ex units", protocol_parameters.max_tx_ex_units.to_string()),
                    label_entry("max block ex units", protocol_parameters.max_block_ex_units.to_string()),
                    label_entry("max value size", protocol_parameters.max_value_size.to_string()),
                    label_entry("max collateral inputs", protocol_parameters.max_collateral_inputs.to_string()),
                ],
            ),
            TuiConfigSection::new(
                "Protocol Parameters · Economic",
                vec![
                    label_entry("min fee a", protocol_parameters.min_fee_a.to_string()),
                    label_entry("min fee b", protocol_parameters.min_fee_b.to_string()),
                    label_entry("stake credential deposit", protocol_parameters.stake_credential_deposit.to_string()),
                    label_entry("stake pool deposit", protocol_parameters.stake_pool_deposit.to_string()),
                    label_entry("monetary expansion", protocol_parameters.monetary_expansion_rate.to_string()),
                    label_entry("treasury expansion", protocol_parameters.treasury_expansion_rate.to_string()),
                    label_entry("min pool cost", protocol_parameters.min_pool_cost.to_string()),
                    label_entry("lovelace per UTxO byte", protocol_parameters.lovelace_per_utxo_byte.to_string()),
                    label_entry("prices", protocol_parameters.prices.to_string()),
                    label_entry("collateral percentage", protocol_parameters.collateral_percentage.to_string()),
                    label_entry(
                        "ref script fee per byte",
                        protocol_parameters.min_fee_ref_script_lovelace_per_byte.to_string(),
                    ),
                    label_entry(
                        "max ref script size per tx",
                        protocol_parameters.max_ref_script_size_per_tx.to_string(),
                    ),
                    label_entry(
                        "max ref script size per block",
                        protocol_parameters.max_ref_script_size_per_block.to_string(),
                    ),
                    label_entry("ref script stride", protocol_parameters.ref_script_cost_stride.to_string()),
                    label_entry("ref script multiplier", protocol_parameters.ref_script_cost_multiplier.to_string()),
                ],
            ),
            TuiConfigSection::new(
                "Protocol Parameters · Governance",
                vec![
                    label_entry(
                        "pool max retirement epoch",
                        protocol_parameters.stake_pool_max_retirement_epoch.to_string(),
                    ),
                    label_entry("optimal stake pools", protocol_parameters.optimal_stake_pools_count.to_string()),
                    label_entry("pledge influence", protocol_parameters.pledge_influence.to_string()),
                    label_entry("min committee size", protocol_parameters.min_committee_size.to_string()),
                    label_entry("max committee term length", protocol_parameters.max_committee_term_length.to_string()),
                    label_entry("gov action lifetime", protocol_parameters.gov_action_lifetime.to_string()),
                    label_entry("gov action deposit", protocol_parameters.gov_action_deposit.to_string()),
                    label_entry("drep deposit", protocol_parameters.drep_deposit.to_string()),
                    label_entry("drep expiry", protocol_parameters.drep_expiry.to_string()),
                ],
            ),
        ]
    }
}

fn config_entry(
    label: &'static str,
    option: &'static str,
    env_var: &'static str,
    value: impl Into<String>,
) -> TuiConfigEntry {
    TuiConfigEntry::new(label, Some(option), Some(env_var), value)
}

fn label_entry(label: &'static str, value: impl Into<String>) -> TuiConfigEntry {
    TuiConfigEntry::new(label, None, None, value)
}

fn bool_value(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

fn default_tui_windows_value() -> String {
    TuiConfig::default().windows.iter().map(|window| format_window(*window)).collect::<Vec<_>>().join(", ")
}

fn era_history_value(network: NetworkName) -> String {
    if network.as_era_history().is_some() { format!("builtin for {network}") } else { "not set".to_string() }
}

fn format_window(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3_600 {
        format!("{}m", seconds / 60)
    } else {
        format!("{}h", seconds / 3_600)
    }
}

fn path_value(path: &Path) -> String {
    path.display().to_string()
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
        return path_value(path);
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
    Runnable::soft(RuntimeKind::Node, move |shutdown, meter_provider| run(args, meter_provider, shutdown))
}

async fn run(
    args: Args,
    meter_provider: Option<SdkMeterProvider>,
    shutdown: ShutdownHandle,
) -> Result<(), Box<dyn std::error::Error>> {
    let _pid_file = optional_pid_file(args.pid_file.clone());

    let config = parse_args(args)?;
    let trace_dump_path = config.trace_dump_path.clone();
    let submit_api_address = config.submit_api_address()?;
    pre_flight_checks()?;

    let metrics = meter_provider.clone().map(track_system_metrics).transpose()?;
    let meter = meter_provider.map(|mp| mp.meter(METRICS_METER_NAME));
    let running = build_and_run_node(config, meter)?;

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

    let era_history =
        network.as_era_history().cloned().map(Ok).unwrap_or_else(|| load_era_history(args.era_history.as_deref()))?;

    let global_parameters = network.as_global_parameters().cloned().unwrap_or(args.global_parameters);

    let ledger_dir = args.ledger_dir.unwrap_or_else(|| default_ledger_dir(network).into());

    let chain_dir = args.chain_dir.unwrap_or_else(|| default_chain_dir(network).into());

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
        mempool,
        tx_submission_responder_params: tx_submission_params,
        ..Config::default()
    })
}

fn load_era_history(path: Option<&Path>) -> Result<EraHistory, Box<dyn std::error::Error>> {
    match path {
        Some(path) => Ok(serde_json::from_slice(&std::fs::read(path)?)?),
        None => Err(anyhow!("missing era history for custom network").into()),
    }
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
