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

//! Embeddable Amaru node runtime.
//!
//! # Embedding
//!
//! Depend on this crate only for steady-state operation (see EDR 031). Cold-start
//! snapshot import lives in `amaru-bootstrap`. The product TUI is never required.
//!
//! ```ignore
//! use amaru_node::{LedgerObservers, NetworkName, NodeBuilder};
//!
//! let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
//! let running = NodeBuilder::new(NetworkName::Preprod)?
//!     .listen_ephemeral_localhost()
//!     .observers(LedgerObservers::new().on_adopted_block(|_block| { /* ... */ }))
//!     .build_and_run(rt.handle())?;
//! // Stop from outside with running.request_abort(); await running.termination().
//! ```
//!
//! The Tokio runtime is always an **explicit** argument — never taken from ambient
//! async context. Metrics are optional ([`NodeBuilder::meter`] / [`Config::meter`]).

pub mod builder;
pub mod chain_realign;
pub mod ledger_reset;
pub mod peer_snapshot;
pub mod stages;
pub mod submit_api;
pub mod telemetry;

pub use amaru_kernel::{Epoch, EraHistory, GlobalParameters, NetworkMagic, NetworkName, Point, Tip, Transaction};
pub use amaru_ledger::{
    AccountState, AdoptedBlock, AdoptedTransaction, DRepState, LedgerBlockEvent, LedgerObservers, LedgerStateSnapshot,
    PoolState, StakeDistribution, StakeSummary, UndoneBlock, UtxoDiff,
};
pub use amaru_metrics::{METRICS_METER_NAME, Meter};
pub use amaru_observability::{FieldValue, TelemetryCaptureLayer, TelemetryRecord, subscribe_telemetry};
pub use builder::{NodeBuilder, default_store_paths, path_is_populated};
pub use chain_realign::{ClearValidity, realign_chain_store_to};
pub use ledger_reset::reset_ledger_to_epoch;
pub use stages::{
    build_node::{NodeRunning, build_and_run_node, build_node, make_state},
    config::{Config, LedgerConfig, MaxExtraLedgerSnapshots, StoreType},
};
pub use telemetry::Telemetry;

/// Default relative path for ledger storage for a known network.
pub fn default_ledger_dir(network: NetworkName) -> String {
    format!("./ledger.{}.db", network.to_string().to_lowercase())
}

/// Default relative path for chain storage for a known network.
pub fn default_chain_dir(network: NetworkName) -> String {
    format!("./chain.{}.db", network.to_string().to_lowercase())
}

/// Default listen address for inbound peer connections.
pub const DEFAULT_LISTEN_ADDRESS: &str = "0.0.0.0:3000";

/// Default public bootstrap peer for mainnet.
pub const MAINNET_DEFAULT_PEER_ADDRESS: &str = "backbone.cardano.iog.io:3001";

/// Default public bootstrap peer for preprod.
pub const PREPROD_DEFAULT_PEER_ADDRESS: &str = "preprod-node.play.dev.cardano.org:3001";

/// Default public bootstrap peer for preview.
pub const PREVIEW_DEFAULT_PEER_ADDRESS: &str = "preview-node.play.dev.cardano.org:3001";

/// Default peer when no network-specific default applies (custom testnets).
pub const DEFAULT_PEER_ADDRESS: &str = "127.0.0.1:3001";

/// Get the default peer address for a given network.
pub fn default_peer_for_network(network: NetworkName) -> &'static str {
    match network {
        NetworkName::Mainnet => MAINNET_DEFAULT_PEER_ADDRESS,
        NetworkName::Preprod => PREPROD_DEFAULT_PEER_ADDRESS,
        NetworkName::Preview => PREVIEW_DEFAULT_PEER_ADDRESS,
        NetworkName::Testnet(_) => DEFAULT_PEER_ADDRESS,
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub mod tests;

pub const DEFAULT_PEER_REMOVAL_COOLDOWN_SECS: u64 = 600; // 10 minutes
pub const DEFAULT_UPSTREAM_PEERS: usize = 3;
pub const DEFAULT_DOWNSTREAM_PEERS: usize = 10;
