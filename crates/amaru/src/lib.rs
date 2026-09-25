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

#[cfg(all(not(target_family = "wasm"), not(target_arch = "riscv32")))]
const _: () = amaru_deps::AMARU_DEPS_USED;

use amaru_kernel::NetworkName;

pub mod exit;
pub mod lifecycle;
pub mod metrics;
pub mod observability;
pub mod panic;
pub mod version;

// Re-export bootstrap for CLI callers; new code should depend on `amaru-bootstrap`.
pub use amaru_bootstrap as bootstrap;
pub use amaru_bootstrap::{
    AnonymousS3Client, DEFAULT_BUCKET, DEFAULT_ENDPOINT, DEFAULT_PUBLIC_URL, DEFAULT_REGION, S3Client, S3Config,
    S3Snapshot, aws, cardano_node, default_snapshots_dir,
};
pub use amaru_node::{
    DEFAULT_LOCAL_PEER_ADDRESS, DEFAULT_MAINNET_PEER_ADDRESS, DEFAULT_PEERS_LISTEN_ON, DEFAULT_PREPROD_PEER_ADDRESS,
    DEFAULT_PREVIEW_PEER_ADDRESS, default_chain_dir, default_ledger_dir, default_peer_for_network,
};

pub const SNAPSHOTS_DIR: &str = amaru_bootstrap::SNAPSHOTS_PATH;

pub const DEFAULT_CONFIG_DIR: &str = "data";

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

    /// A byte size, either a raw count or a unit such as `100MiB` or `10kB`.
    pub const BYTE_SIZE: &str = "BYTE_SIZE";

    /// For directories / folders on the filesystem.
    pub const DIRECTORY: &str = "DIR";

    /// A positive duration, with common units for milliseconds, seconds, minutes, or hours.
    pub const DURATION: &str = "UINT[ms|s|min|h]";

    /// For network addresses made of an hostname and an option port number. Also known as an
    /// _authority_.
    pub const ENDPOINT: &str = "HOSTNAME[:PORT]";

    /// For filepaths on the file-system.
    pub const FILEPATH: &str = "FILEPATH";

    /// Designates a well-known Cardano network name, or a custom dev network.
    pub const NETWORK: &str = "mainnet|preprod|preview|testnet_<U32>";

    /// A formula for describing the desired mix of peers to connect to.
    pub const PEER_MIX: &str = "PEER_MIX";

    /// A blockchain point, formatted as slot.hash
    pub const POINT: &str = "SLOT.HEADER_HASH";

    /// A blockchain point, formatted as slot.hash
    pub const POINT_OR_HASH: &str = "SLOT.HEADER_HASH or HEADER_HASH";

    /// A snapshot point identifying the last point of an epoch and its parent.
    pub const SNAPSHOT: &str = "SLOT.HEADER_HASH::PARENT_SLOT.PARENT_HEADER_HASH";

    /// For S3-compatible regions, including Cloudflare R2's `auto` region.
    pub const S3_REGION: &str = "auto|REGION";

    /// A key/value string pair.
    pub const STR_KEY_VALUE: &str = "KEY=value";

    /// A non-negative integer value.
    pub const UINT: &str = "UINT";

    /// A non-negative integer value, or the keyword `all`.
    pub const UINT_ALL: &str = "UINT|all";

    /// For HTTP or HTTPS URLs.
    pub const URL: &str = "URL";
}

/// Environment variables used across command-line options.
pub mod env_vars {
    pub const CARDANO_NODE_CONFIG: &str = "AMARU_CARDANO_NODE_CONFIG";
    pub const CARDANO_NODE_DB: &str = "AMARU_CARDANO_NODE_DB";
    pub const CHAIN_DB: &str = "AMARU_CHAIN_DB";
    pub const DIST: &str = "AMARU_DIST";
    pub const EPOCH: &str = "AMARU_EPOCH";
    pub const ERA_HISTORY: &str = "AMARU_ERA_HISTORY";
    pub const HEADERS: &str = "AMARU_HEADERS";
    pub const LEDGER_DB: &str = "AMARU_LEDGER_DB";
    pub const LEDGER_MAX_EXTRA_SNAPSHOTS: &str = "AMARU_LEDGER_MAX_EXTRA_SNAPSHOTS";
    pub const MIGRATE_CHAIN_DB: &str = "AMARU_MIGRATE_CHAIN_DB";
    pub const MITHRIL_MAX_BLOCKS: &str = "AMARU_MITHRIL_MAX_BLOCKS";
    pub const MITHRIL_SNAPSHOTS: &str = "AMARU_MITHRIL_SNAPSHOTS";
    pub const MITHRIL_UNTIL_SLOT: &str = "AMARU_MITHRIL_UNTIL_SLOT";
    pub const NETWORK: &str = "AMARU_NETWORK";
    pub const NO_TUI: &str = "AMARU_NO_TUI";
    pub const PARENT: &str = "AMARU_PARENT";
    pub const PEER: &str = "AMARU_PEER";
    pub const PEERS_LISTEN_ON: &str = "AMARU_PEERS_LISTEN_ON";
    pub const PEERS_MAX_DOWNSTREAM: &str = "AMARU_PEERS_MAX_DOWNSTREAM";
    pub const PEERS_MAX_UPSTREAM: &str = "AMARU_PEERS_MAX_UPSTREAM";
    pub const PEER_MIX: &str = "AMARU_PEER_MIX";
    pub const PEER_REMOVAL_COOLDOWN: &str = "AMARU_PEER_REMOVAL_COOLDOWN";
    pub const PEERS_SNAPSHOT: &str = "AMARU_PEERS_SNAPSHOT";
    pub const PID_FILE: &str = "AMARU_PID_FILE";
    pub const SNAPSHOT: &str = "AMARU_SNAPSHOT";
    pub const SNAPSHOTS: &str = "AMARU_SNAPSHOTS";
    pub const SUBMIT_API_LISTEN_ON: &str = "AMARU_SUBMIT_API_LISTEN_ON";
    pub const TRACE_BUFFER: &str = "AMARU_TRACE_BUFFER";
    pub const TRACE_BUFFER_DUMP: &str = "AMARU_TRACE_BUFFER_DUMP";
    pub const TUI_LOG_RETENTION: &str = "AMARU_TUI_LOG_RETENTION";
}
