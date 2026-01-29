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

use crate::bootstrap::{BootstrapError, InitialNonces};
use amaru_kernel::NetworkName;
use include_dir::{Dir, include_dir};
use std::{error::Error, path::PathBuf};

pub mod bootstrap;
pub mod exit;
pub mod metrics;
pub mod observability;
pub mod panic;

/// Sync pipeline
///
/// The sync pipeline is responsible for fetching blocks from the upstream node and
/// applying them to the local chain.
pub mod stages;

pub const SNAPSHOTS_DIR: &str = "snapshots";

pub const DEFAULT_NETWORK: NetworkName = NetworkName::Preprod;

pub const DEFAULT_PEER_ADDRESS: &str = "127.0.0.1:3001";

/// Default address to listen on for incoming connections.
pub const DEFAULT_LISTEN_ADDRESS: &str = "0.0.0.0:3000";

pub const DEFAULT_CONFIG_DIR: &str = "data";

const SNAPSHOTS_PATH: &str = "snapshots";
const BOOTSTRAP_PATH: &str = "crates/amaru/config/bootstrap";
static BOOTSTRAP_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/config/bootstrap");

pub fn default_ledger_dir(network: NetworkName) -> String {
    format!("./ledger.{}.db", network.to_string().to_lowercase())
}

pub fn default_chain_dir(network: NetworkName) -> String {
    format!("./chain.{}.db", network.to_string().to_lowercase())
}

pub fn bootstrap_config_dir(network: NetworkName) -> PathBuf {
    format!("{}/{}", BOOTSTRAP_PATH, network.to_string().to_lowercase()).into()
}

pub fn default_snapshots_dir(network: NetworkName) -> String {
    format!("{}/{}", SNAPSHOTS_PATH, network.to_string().to_lowercase())
}

pub fn default_data_dir(network: NetworkName) -> String {
    format!(
        "{}/{}",
        DEFAULT_CONFIG_DIR,
        network.to_string().to_lowercase()
    )
}

pub fn default_initial_nonces(network: NetworkName) -> Result<InitialNonces, Box<dyn Error>> {
    let default_nonces_file_name = "nonces.json";

    let nonces_file = get_bootstrap_file(network, default_nonces_file_name)?.ok_or(
        BootstrapError::MissingConfigFile(default_nonces_file_name.into()),
    )?;

    Ok(serde_json::from_slice(nonces_file.as_slice())?)
}

pub fn get_bootstrap_file(
    network: NetworkName,
    name: &str,
) -> Result<Option<Vec<u8>>, Box<dyn Error>> {
    let path = format!("{}/{}", network.to_string().to_lowercase(), name);
    Ok(BOOTSTRAP_DIR.get_file(path).map(|f| f.contents().into()))
}

pub fn get_bootstrap_headers(
    network: NetworkName,
) -> Result<impl Iterator<Item = Vec<u8>>, Box<dyn Error>> {
    let path = format!("{}/headers/*", network.to_string().to_lowercase());
    Ok(BOOTSTRAP_DIR
        .find(&path)?
        .filter_map(|f| f.as_file())
        .map(|f| f.contents().into()))
}

/// Value names (a.k.a. metavar) used across command-line options.
///
/// Conventions:
///
/// - Uppercase for types
/// - Lowercase for enums / verbatim values
pub mod value_names {
    /// For directories / folders on the filesystem.
    pub const DIRECTORY: &str = "DIR";

    /// For network addresses made of an hostname and an option port number. Also known as an
    /// _authority_.
    pub const ENDPOINT: &str = "HOSTNAME[:PORT]";

    /// For filepaths on the file-system.
    pub const FILEPATH: &str = "FILEPATH";

    /// Designates a well-known Cardano network name, or a custom dev network.
    pub const NETWORK: &str = "mainnet|preprod|preview|testnet_<U32>";

    /// A blockchain point, formatted as slot.hash
    pub const POINT: &str = "SLOT.HEADER_HASH";

    /// A non-negative integer value.
    pub const UINT: &str = "UINT";

    /// A non-negative integer value, or the keyword 'all'
    pub const UINT_ALL: &str = "UINT|all";
}

/// Environment variables used across command-line options.
pub mod env_vars {
    /// --chain-dir
    pub const CHAIN_DIR: &str = "AMARU_CHAIN_DIR";

    /// --epoch
    pub const EPOCH: &str = "AMARU_EPOCH";

    /// --header-file
    pub const HEADER_FILE: &str = "AMARU_HEADER_FILE";

    /// --headers-dir
    pub const HEADERS_DIR: &str = "AMARU_HEADERS_DIR";

    /// --ledger-dir
    pub const LEDGER_DIR: &str = "AMARU_LEDGER_DIR";

    /// --listen-address
    pub const LISTEN_ADDRESS: &str = "AMARU_LISTEN_ADDRESS";

    /// --max-downstream-peers
    pub const MAX_DOWNSTREAM_PEERS: &str = "AMARU_MAX_DOWNSTREAM_PEERS";

    /// --max-extra-ledger-snapshots
    pub const MAX_EXTRA_LEDGER_SNAPSHOTS: &str = "AMARU_MAX_EXTRA_LEDGER_SNAPSHOTS";

    /// --migrate-chain-db
    pub const MIGRATE_CHAIN_DB: &str = "AMARU_MIGRATE_CHAIN_DB";

    /// --network
    pub const NETWORK: &str = "AMARU_NETWORK";

    /// --nonces-file
    pub const NONCES_FILE: &str = "AMARU_NONCES_FILE";

    /// --parent
    pub const PARENT: &str = "AMARU_PARENT";

    /// --peer-address
    pub const PEER_ADDRESS: &str = "AMARU_PEER_ADDRESS";

    /// --pid-file
    pub const PID_FILE: &str = "AMARU_PID_FILE";

    /// --snapshot
    pub const SNAPSHOT: &str = "AMARU_SNAPSHOT";

    /// --snapshots-dir
    pub const SNAPSHOTS_DIR: &str = "AMARU_SNAPSHOTS_DIR";

    /// --target-dir
    pub const TARGET_DIR: &str = "AMARU_TARGET_DIR";
}
