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

use amaru_kernel::NetworkName;

pub mod aws;
pub mod bootstrap;
pub mod cardano_node;
pub mod exit;
pub mod lifecycle;
pub mod metrics;
pub mod observability;
pub mod panic;
pub mod version;

pub const SNAPSHOTS_DIR: &str = "snapshots";

pub const DEFAULT_PEER_ADDRESS: &str = "127.0.0.1:3001";

/// Default public bootstrap peer for mainnet.
pub const MAINNET_DEFAULT_PEER_ADDRESS: &str = "backbone.cardano.iog.io:3001";

/// Default public bootstrap peer for preprod.
pub const PREPROD_DEFAULT_PEER_ADDRESS: &str = "preprod-node.play.dev.cardano.org:3001";

/// Default public bootstrap peer for preview.
pub const PREVIEW_DEFAULT_PEER_ADDRESS: &str = "preview-node.play.dev.cardano.org:3001";

/// Default address to listen on for incoming connections.
pub const DEFAULT_LISTEN_ADDRESS: &str = "0.0.0.0:3000";

pub const DEFAULT_CONFIG_DIR: &str = "data";

const SNAPSHOTS_PATH: &str = "snapshots";

pub fn default_ledger_dir(network: NetworkName) -> String {
    format!("./ledger.{}.db", network.to_string().to_lowercase())
}

pub fn default_chain_dir(network: NetworkName) -> String {
    format!("./chain.{}.db", network.to_string().to_lowercase())
}

pub fn default_snapshots_dir(network: NetworkName) -> String {
    format!("{}/{}", SNAPSHOTS_PATH, network.to_string().to_lowercase())
}

pub fn default_data_dir(network: NetworkName) -> String {
    format!("{}/{}", DEFAULT_CONFIG_DIR, network.to_string().to_lowercase())
}

/// Value names (a.k.a. metavar) used across command-line options.
///
/// Conventions:
///
/// - Uppercase for types
/// - Lowercase for enums / verbatim values
pub mod value_names {
    /// For S3-compatible bucket names.
    pub const BUCKET_NAME: &str = "BUCKET_NAME";

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

    /// A blockchain point, formatted as slot.hash
    pub const POINT_OR_HASH: &str = "SLOT.HEADER_HASH or HEADER_HASH";

    /// A snapshot point identifying the last point of an epoch and its parent.
    pub const SNAPSHOT: &str = "SLOT.HEADER_HASH::PARENT_SLOT.PARENT_HEADER_HASH";

    /// For S3-compatible regions, including Cloudflare R2's `auto` region.
    pub const S3_REGION: &str = "auto|REGION";

    /// A non-negative integer value.
    pub const UINT: &str = "UINT";

    /// A non-negative integer value, or the keyword 'all'
    pub const UINT_ALL: &str = "UINT|all";

    /// For HTTP or HTTPS URLs.
    pub const URL: &str = "URL";
}

/// Get the default peer address for a given network.
pub fn default_peer_for_network(network: NetworkName) -> &'static str {
    match network {
        NetworkName::Mainnet => MAINNET_DEFAULT_PEER_ADDRESS,
        NetworkName::Preprod => PREPROD_DEFAULT_PEER_ADDRESS,
        NetworkName::Preview => PREVIEW_DEFAULT_PEER_ADDRESS,
        NetworkName::Testnet(_) => DEFAULT_PEER_ADDRESS, // localhost for custom networks
    }
}

/// Environment variables used across command-line options.
pub mod env_vars {
    /// --cardano-node-config-dir
    pub const CARDANO_NODE_CONFIG_DIR: &str = "AMARU_CARDANO_NODE_CONFIG_DIR";

    /// --cardano-node-db
    pub const CARDANO_NODE_DB: &str = "AMARU_CARDANO_NODE_DB";

    /// --chain-dir
    pub const CHAIN_DIR: &str = "AMARU_CHAIN_DIR";

    /// --dist-dir
    pub const DIST_DIR: &str = "AMARU_DIST_DIR";

    /// --epoch
    pub const EPOCH: &str = "AMARU_EPOCH";

    /// --header-file
    pub const HEADER_FILE: &str = "AMARU_HEADER_FILE";

    /// --headers-dir
    pub const HEADERS_DIR: &str = "AMARU_HEADERS_DIR";

    /// --ledger-dir
    pub const LEDGER_DIR: &str = "AMARU_LEDGER_DIR";

    /// --era-history
    pub const ERA_HISTORY: &str = "AMARU_ERA_HISTORY";

    /// --listen-address
    pub const LISTEN_ADDRESS: &str = "AMARU_LISTEN_ADDRESS";

    /// --downstream-peers
    pub const DOWNSTREAM_PEERS: &str = "AMARU_DOWNSTREAM_PEERS";

    /// --upstream-peers
    pub const UPSTREAM_PEERS: &str = "AMARU_UPSTREAM_PEERS";

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

    /// --peer-snapshot
    pub const PEER_SNAPSHOT: &str = "AMARU_PEER_SNAPSHOT";

    /// --peer-removal-cooldown-secs
    pub const PEER_REMOVAL_COOLDOWN_SECS: &str = "AMARU_PEER_REMOVAL_COOLDOWN_SECS";

    /// --pid-file
    pub const PID_FILE: &str = "AMARU_PID_FILE";

    /// --snapshot
    pub const SNAPSHOT: &str = "AMARU_SNAPSHOT";

    /// --snapshots-dir
    pub const SNAPSHOTS_DIR: &str = "AMARU_SNAPSHOTS_DIR";

    /// --submit-api-address
    pub const SUBMIT_API_ADDRESS: &str = "AMARU_SUBMIT_API_ADDRESS";

    /// --trace-buffer
    pub const TRACE_BUFFER: &str = "AMARU_TRACE_BUFFER";

    /// --dump-trace-buffer
    pub const DUMP_TRACE_BUFFER: &str = "AMARU_DUMP_TRACE_BUFFER";

    /// --target-dir
    pub const TARGET_DIR: &str = "AMARU_TARGET_DIR";
}
