// Copyright 2026 PRAGMA
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

//! Opinionated construction of a node [`Config`] and [`NodeRunning`] handle.
//!
//! Prefer this over assembling [`Config`] field-by-field. Advanced knobs remain
//! available on the finished [`Config`] if needed.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use amaru_kernel::{EraHistory, GlobalParameters, NetworkName};
use amaru_ledger::LedgerObservers;
use amaru_metrics::Meter;
use amaru_stores::rocksdb::RocksDbConfig;
use anyhow::{Context, bail};
use tokio::runtime::Handle;

use crate::{
    DEFAULT_LISTEN_ADDRESS, default_chain_dir, default_ledger_dir, default_peer_for_network,
    peer_snapshot::load_embedded_peer_snapshot,
    stages::{
        build_node::{NodeRunning, build_and_run_node},
        config::{Config, LedgerConfig, MaxExtraLedgerSnapshots, StoreType},
    },
};

/// Fluent builder for the common embedding path.
///
/// ```ignore
/// let running = NodeBuilder::new(NetworkName::Preprod)?
///     .peer(default_peer_for_network(NetworkName::Preprod))
///     .observers(LedgerObservers::new().on_adopted_block(|_| {}))
///     .build_and_run(&runtime.handle())?;
/// ```
#[derive(Clone)]
pub struct NodeBuilder {
    network: NetworkName,
    ledger_dir: PathBuf,
    chain_dir: PathBuf,
    upstream_peers: Vec<String>,
    use_default_peer_if_empty: bool,
    load_embedded_peer_snapshot: bool,
    target_upstream_peers: Option<usize>,
    target_downstream_peers: Option<usize>,
    listen_address: String,
    submit_api_address: Option<String>,
    migrate_chain_db: bool,
    max_extra_ledger_snapshots: MaxExtraLedgerSnapshots,
    era_history: Option<EraHistory>,
    global_parameters: Option<GlobalParameters>,
    observers: LedgerObservers,
    meter: Option<Arc<Meter>>,
}

impl NodeBuilder {
    /// Start a builder for a known network profile.
    ///
    /// Era history and global parameters are taken from the network. For custom
    /// `NetworkName::Testnet(_)`, call [`Self::era_history`] and
    /// [`Self::global_parameters`] before [`Self::build`].
    pub fn new(network: NetworkName) -> anyhow::Result<Self> {
        Ok(Self {
            network,
            ledger_dir: PathBuf::from(default_ledger_dir(network)),
            chain_dir: PathBuf::from(default_chain_dir(network)),
            upstream_peers: Vec::new(),
            use_default_peer_if_empty: true,
            load_embedded_peer_snapshot: true,
            target_upstream_peers: None,
            target_downstream_peers: None,
            listen_address: DEFAULT_LISTEN_ADDRESS.to_string(),
            submit_api_address: None,
            migrate_chain_db: false,
            max_extra_ledger_snapshots: MaxExtraLedgerSnapshots::default(),
            era_history: network.as_era_history().cloned(),
            global_parameters: network.as_global_parameters().cloned(),
            observers: LedgerObservers::default(),
            meter: None,
        })
    }

    pub fn ledger_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.ledger_dir = path.into();
        self
    }

    pub fn chain_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.chain_dir = path.into();
        self
    }

    /// Replace upstream peers (clears any previously set list).
    pub fn peers<I, S>(mut self, peers: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.upstream_peers = peers.into_iter().map(Into::into).collect();
        self.use_default_peer_if_empty = false;
        self
    }

    /// Append a single upstream peer.
    pub fn peer(mut self, peer: impl Into<String>) -> Self {
        self.upstream_peers.push(peer.into());
        self.use_default_peer_if_empty = false;
        self
    }

    /// When no peers were set, do not fall back to the network default bootstrap peer.
    pub fn no_default_peer(mut self) -> Self {
        self.use_default_peer_if_empty = false;
        self
    }

    /// Skip loading the build-time embedded big-ledger peer snapshot.
    pub fn without_embedded_peer_snapshot(mut self) -> Self {
        self.load_embedded_peer_snapshot = false;
        self
    }

    pub fn target_upstream_peers(mut self, n: usize) -> Self {
        self.target_upstream_peers = Some(n);
        self
    }

    pub fn target_downstream_peers(mut self, n: usize) -> Self {
        self.target_downstream_peers = Some(n);
        self
    }

    pub fn listen_address(mut self, addr: impl Into<String>) -> Self {
        self.listen_address = addr.into();
        self
    }

    /// Bind only on loopback with an ephemeral port (typical for headless tools).
    pub fn listen_ephemeral_localhost(self) -> Self {
        self.listen_address("127.0.0.1:0")
    }

    pub fn submit_api_address(mut self, addr: impl Into<String>) -> Self {
        self.submit_api_address = Some(addr.into());
        self
    }

    pub fn migrate_chain_db(mut self, migrate: bool) -> Self {
        self.migrate_chain_db = migrate;
        self
    }

    pub fn max_extra_ledger_snapshots(mut self, max: MaxExtraLedgerSnapshots) -> Self {
        self.max_extra_ledger_snapshots = max;
        self
    }

    pub fn era_history(mut self, era_history: EraHistory) -> Self {
        self.era_history = Some(era_history);
        self
    }

    pub fn global_parameters(mut self, global_parameters: GlobalParameters) -> Self {
        self.global_parameters = Some(global_parameters);
        self
    }

    pub fn observers(mut self, observers: LedgerObservers) -> Self {
        self.observers = observers;
        self
    }

    /// Metrics sink (OpenTelemetry / local observer). Defaults to an empty [`Meter`].
    pub fn meter(mut self, meter: Arc<Meter>) -> Self {
        self.meter = Some(meter);
        self
    }

    /// Finish a full [`Config`] (for inspection or further field tweaks).
    pub fn build(self) -> anyhow::Result<Config> {
        let era_history = self.era_history.ok_or_else(|| {
            anyhow::anyhow!("era history is required for network {}; provide NodeBuilder::era_history", self.network)
        })?;
        let global_parameters = self.global_parameters.ok_or_else(|| {
            anyhow::anyhow!(
                "global parameters are required for network {}; provide NodeBuilder::global_parameters",
                self.network
            )
        })?;

        let mut upstream_peers = self.upstream_peers;
        if upstream_peers.is_empty() && self.use_default_peer_if_empty {
            upstream_peers.push(default_peer_for_network(self.network).to_string());
        }
        if upstream_peers.is_empty() {
            bail!("at least one upstream peer is required");
        }

        let peer_snapshot_peers = if self.load_embedded_peer_snapshot {
            load_embedded_peer_snapshot(self.network)
                .context("load embedded peer snapshot")?
                .map(|s| s.peers)
                .unwrap_or_default()
        } else {
            Default::default()
        };

        let mut config = Config {
            ledger_config: LedgerConfig {
                ledger_store: RocksDbConfig::new(self.ledger_dir),
                network: self.network,
                era_history,
                global_parameters,
                max_extra_ledger_snapshots: self.max_extra_ledger_snapshots,
                ..LedgerConfig::default()
            },
            chain_store: StoreType::RocksDb(RocksDbConfig::new(self.chain_dir)),
            upstream_peers,
            peer_snapshot_peers,
            network_magic: self.network.to_network_magic(),
            listen_address: self.listen_address,
            migrate_chain_db: self.migrate_chain_db,
            submit_api_address: self.submit_api_address,
            observers: self.observers,
            meter: self.meter,
            ..Config::default()
        };

        if let Some(n) = self.target_upstream_peers {
            config.target_upstream_peers = n;
        }
        if let Some(n) = self.target_downstream_peers {
            config.target_downstream_peers = n;
        }

        Ok(config)
    }

    /// Build the config and start the node on the given Tokio runtime handle.
    ///
    /// The runtime is **not** taken from ambient context: pass an explicit
    /// [`Handle`] (for example `runtime.handle()` or `Handle::current()` when
    /// you are already inside that runtime).
    pub fn build_and_run(self, runtime: &Handle) -> anyhow::Result<NodeRunning> {
        let config = self.build()?;
        build_and_run_node(config, runtime)
    }
}

/// Convenience: default ledger/chain dirs for `network` as paths.
pub fn default_store_paths(network: NetworkName) -> (PathBuf, PathBuf) {
    (PathBuf::from(default_ledger_dir(network)), PathBuf::from(default_chain_dir(network)))
}

/// True if `path` exists and is non-empty (same idea as `amaru node bootstrap` guards).
pub fn path_is_populated(path: &Path) -> std::io::Result<bool> {
    match std::fs::read_dir(path) {
        Ok(mut entries) => Ok(entries.next().is_some()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}
