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
    fmt::Display,
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime},
};

use amaru_consensus::performance::PeerMix;
use amaru_kernel::{
    ConsensusParameters, EraHistory, GlobalParameters, NetworkMagic, NetworkName, PREPROD_ERA_HISTORY,
    PREPROD_GLOBAL_PARAMETERS, Peer, PeerCandidate,
};
use amaru_mempool::MempoolConfig;
use amaru_metrics::Meter;
use amaru_ouroboros::ChainStore;
use amaru_protocols::tx_submission::ResponderParams;
use amaru_pure_stage::Instant;
use amaru_stores::rocksdb::RocksDbConfig;
use anyhow::Context;

use crate::{DEFAULT_DOWNSTREAM_PEERS, DEFAULT_PEER_REMOVAL_COOLDOWN_SECS, DEFAULT_UPSTREAM_PEERS};

/// Configuration for the Amaru node, including storage options, network settings, and other parameters.
///
/// Prefer [`crate::NodeBuilder`] for embedding; this struct remains the full explicit surface
/// used by the CLI, tests, and advanced callers.
pub struct Config {
    pub ledger_config: LedgerConfig,
    pub chain_store: StoreType<Arc<dyn ChainStore>>,
    pub upstream_peers: Vec<String>,
    /// Big-ledger relays from a Cardano peer snapshot file (`--peer-snapshot`).
    pub peer_snapshot_peers: BTreeSet<Peer>,
    /// Snapshot relays that still need DNS (hostname or SRV).
    pub peer_snapshot_unresolved: BTreeSet<PeerCandidate>,
    pub target_upstream_peers: usize,
    pub target_downstream_peers: usize,
    /// Outbound source mix formula (EDR-031); default prefers static floor then shared/snapshot/ledger.
    pub peer_mix: PeerMix,
    pub network_magic: NetworkMagic,
    pub listen_address: String,
    pub migrate_chain_db: bool,
    pub submit_api_address: Option<String>,

    /// After a misbehaving upstream peer is removed, do not allow it to be re-added for this many seconds.
    pub peer_removal_cooldown_secs: u64,

    /// Maximum distance (in block height) below the adopted tip for which `block_source` retains provenance.
    pub block_source_max_tip_distance: u64,

    /// Minimum number of trace entries retained when the stage graph trace buffer is full.
    pub trace_buffer_min_entries: usize,

    /// Maximum total size in bytes of CBOR trace entries in the stage graph trace buffer.
    pub trace_buffer_max_size: usize,

    /// If set, raw trace buffer bytes are written here during node shutdown.
    pub trace_dump_path: Option<PathBuf>,

    /// Mempool configuration (max size for now).
    pub mempool: MempoolConfig,

    /// Tx-submission responder parameters (max outstanding tx-id window, fetch batch size, etc...).
    pub tx_submission_responder_params: ResponderParams,

    /// Optional embedder observers (adopted blocks, full stake summaries).
    pub observers: amaru_ledger::LedgerObservers,

    /// Metrics sink. When `None`, [`build_and_run_node`](crate::build_and_run_node) uses
    /// [`Meter::default`] (no OpenTelemetry export, no local observer).
    pub meter: Option<Arc<Meter>>,
}

impl Config {
    /// Parse the listen address into a `SocketAddr`.
    pub fn listen_address(&self) -> anyhow::Result<SocketAddr> {
        self.listen_address.parse().context("invalid listen address")
    }

    /// Parse the optional submit API address into a `SocketAddr`.
    pub fn submit_api_address(&self) -> anyhow::Result<Option<SocketAddr>> {
        self.submit_api_address.as_deref().map(|addr| addr.parse().context("invalid submit API address")).transpose()
    }

    pub fn network(&self) -> NetworkName {
        self.ledger_config.network
    }

    pub fn era_history(&self) -> &EraHistory {
        self.ledger_config.era_history()
    }

    pub fn global_parameters(&self) -> &GlobalParameters {
        &self.ledger_config.global_parameters
    }

    /// The global clock offset for real-time node execution (i.e. not in a simulation test)
    /// needs to be the difference between the `GlobalParameters::start_time` and the pure-stage EPOCH
    #[expect(clippy::expect_used)]
    pub fn compute_global_clock_offset(&self) -> Duration {
        let system_time = SystemTime::now();
        // calling `.duration_since_global_epoch()` ensures that EPOCH is initialized
        // and constructing the Instant with `Duration::ZERO` returns `now-EPOCH`
        let duration_since_epoch =
            Instant::from_tokio(tokio::time::Instant::now(), Duration::ZERO).duration_since_global_epoch();
        let system_start = SystemTime::UNIX_EPOCH
            .checked_add(Duration::from_millis(self.global_parameters().system_start))
            .expect("System start time must be valid POSIX time");
        system_time
            .duration_since(system_start)
            .expect("Process start must be after Ouroboros system start time")
            .checked_sub(duration_since_epoch)
            .expect("Process EPOCH must be after the UNIX_EPOCH")
    }
}

impl Default for Config {
    fn default() -> Config {
        Config {
            ledger_config: LedgerConfig::default(),
            chain_store: StoreType::RocksDb(RocksDbConfig::new(PathBuf::from("./chain.db"))),
            upstream_peers: vec![],
            peer_snapshot_peers: BTreeSet::new(),
            peer_snapshot_unresolved: BTreeSet::new(),
            target_upstream_peers: DEFAULT_UPSTREAM_PEERS,
            target_downstream_peers: DEFAULT_DOWNSTREAM_PEERS,
            peer_mix: PeerMix::default(),
            network_magic: NetworkMagic::PREPROD,
            listen_address: "0.0.0.0:3000".to_string(),
            migrate_chain_db: false,
            submit_api_address: None,
            peer_removal_cooldown_secs: DEFAULT_PEER_REMOVAL_COOLDOWN_SECS,
            block_source_max_tip_distance: 2_500,
            trace_buffer_min_entries: 0,
            trace_buffer_max_size: 0,
            trace_dump_path: None,
            mempool: MempoolConfig::default(),
            tx_submission_responder_params: ResponderParams::default(),
            observers: amaru_ledger::LedgerObservers::default(),
            meter: None,
        }
    }
}

pub struct LedgerConfig {
    pub ledger_store: RocksDbConfig,
    pub network: NetworkName,
    pub global_parameters: GlobalParameters,
    pub era_history: EraHistory,
    pub max_extra_ledger_snapshots: MaxExtraLedgerSnapshots,
    pub emit_initial_stake_distribution_progress_ticks: bool,
    // Number of allocation arenas to keep around for performing parallel evaluation of scripts in
    // the ledger.
    pub ledger_vm_alloc_arena_count: usize,

    // Initial size (in bytes) of each allocation arena to use for script evaluation in the ledger
    // virtual machine. Higher sizes means less re-allocations but more resident memory footprint
    // since the arena is leaking memory on purpose.
    pub ledger_vm_alloc_arena_size: usize,
}

impl LedgerConfig {
    pub fn to_consensus_parameters(&self) -> ConsensusParameters {
        ConsensusParameters::new(self.global_parameters.clone(), self.era_history())
    }

    pub fn era_history(&self) -> &EraHistory {
        &self.era_history
    }
}

impl Default for LedgerConfig {
    fn default() -> LedgerConfig {
        LedgerConfig {
            ledger_store: RocksDbConfig::new(PathBuf::from("./ledger.db")),
            network: NetworkName::Preprod,
            era_history: PREPROD_ERA_HISTORY.clone(),
            global_parameters: PREPROD_GLOBAL_PARAMETERS.clone(),
            max_extra_ledger_snapshots: MaxExtraLedgerSnapshots::default(),
            emit_initial_stake_distribution_progress_ticks: false,
            ledger_vm_alloc_arena_count: 3,
            ledger_vm_alloc_arena_size: 20_971_520,
        }
    }
}

/// Whether or not data is stored on disk or in memory.
#[derive(Clone)]
pub enum StoreType<S> {
    InMem(S),
    RocksDb(RocksDbConfig),
}

impl<S> Display for StoreType<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreType::InMem(..) => write!(f, "<mem>"),
            StoreType::RocksDb(config) => write!(f, "{}", config),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum MaxExtraLedgerSnapshots {
    All,
    UpTo(u64),
}

impl Default for MaxExtraLedgerSnapshots {
    fn default() -> Self {
        Self::UpTo(0)
    }
}

impl std::fmt::Display for MaxExtraLedgerSnapshots {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::All => f.write_str("all"),
            Self::UpTo(n) => write!(f, "{n}"),
        }
    }
}

impl std::str::FromStr for MaxExtraLedgerSnapshots {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "all" => Ok(Self::All),
            _ => match s.parse() {
                Ok(e) => Ok(Self::UpTo(e)),
                Err(e) => Err(format!("invalid max ledger snapshot, cannot parse value: {e}")),
            },
        }
    }
}

impl From<MaxExtraLedgerSnapshots> for u64 {
    fn from(max_extra_ledger_snapshots: MaxExtraLedgerSnapshots) -> Self {
        match max_extra_ledger_snapshots {
            MaxExtraLedgerSnapshots::All => u64::MAX,
            MaxExtraLedgerSnapshots::UpTo(n) => n,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use amaru_stores::rocksdb::RocksDbConfig;

    use super::StoreType;

    #[test]
    fn test_store_path_display() {
        assert_eq!(format!("{}", StoreType::InMem(())), "<mem>");
        assert_eq!(
            format!("{}", StoreType::<()>::RocksDb(RocksDbConfig::new(PathBuf::from("/path/to/store")))),
            "RocksDbConfig { dir: /path/to/store }"
        );
        assert_eq!(
            format!("{}", StoreType::<()>::RocksDb(RocksDbConfig::new(PathBuf::from("./relative/path")))),
            "RocksDbConfig { dir: ./relative/path }"
        );
        assert_eq!(
            format!("{}", StoreType::<()>::RocksDb(RocksDbConfig::new(PathBuf::from("")))),
            "RocksDbConfig { dir:  }"
        );
    }
}
