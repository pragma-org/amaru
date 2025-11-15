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

use amaru_kernel::network::NetworkName;

pub mod metrics;
pub mod observability;
pub mod panic;
pub mod point;

/// Sync pipeline
///
/// The sync pipeline is responsible for fetching blocks from the upstream node and
/// applying them to the local chain.
pub mod stages;

/// Generic exit handler
pub mod exit;

pub const SNAPSHOTS_DIR: &str = "snapshots";

pub const DEFAULT_NETWORK: NetworkName = NetworkName::Preprod;

pub const DEFAULT_PEER_ADDRESS: &str = "127.0.0.1:3001";

/// Default address to listen on for incoming connections.
pub const DEFAULT_LISTEN_ADDRESS: &str = "0.0.0.0:3000";

pub const DEFAULT_CONFIG_DIR: &str = "data";

pub fn default_ledger_dir(network: NetworkName) -> String {
    format!("./ledger.{}.db", network.to_string().to_lowercase())
}

pub fn default_chain_dir(network: NetworkName) -> String {
    format!("./chain.{}.db", network.to_string().to_lowercase())
}

pub fn default_data_dir(network: NetworkName) -> String {
    format!(
        "{}/{}",
        DEFAULT_CONFIG_DIR,
        network.to_string().to_lowercase()
    )
}

pub fn default_snapshots_dir(network: NetworkName) -> String {
    format!("{}/{}", SNAPSHOTS_DIR, network)
}