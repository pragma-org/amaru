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

use std::{fmt::Display, net::SocketAddr, path::PathBuf, sync::Arc};

use amaru_kernel::{BlockHeader, NetworkMagic, NetworkName};
use amaru_mempool::MempoolConfig;
use amaru_ouroboros::ChainStore;
use amaru_protocols::tx_submission::ResponderParams;
use amaru_stores::{in_memory::MemoryStore, rocksdb::RocksDbConfig};
use anyhow::Context;

/// Configuration for the Amaru node, including storage options, network settings, and other parameters.
pub struct Config {
    pub ledger_store: StoreType<MemoryStore>,
    pub chain_store: StoreType<Arc<dyn ChainStore<BlockHeader>>>,
    pub upstream_peers: Vec<String>,
    pub network: NetworkName,
    pub network_magic: NetworkMagic,
    pub listen_address: String,
    pub max_downstream_peers: usize,
    pub max_extra_ledger_snapshots: MaxExtraLedgerSnapshots,
    pub migrate_chain_db: bool,
    pub submit_api_address: Option<String>,

    // Number of allocation arenas to keep around for performing parallel evaluation of scripts in
    // the ledger.
    pub ledger_vm_alloc_arena_count: usize,

    // Initial size (in bytes) of each allocation arena to use for script evaluation in the ledger
    // virtual machine. Higher sizes means less re-allocations but more resident memory footprint
    // since the arena is leaking memory on purpose.
    pub ledger_vm_alloc_arena_size: usize,

    /// How often the `defer_req_next` stage polls the ledger to dispatch deferred `RequestNext` messages.
    pub defer_req_next_poll_ms: u64,

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
}

impl Default for Config {
    fn default() -> Config {
        Config {
            ledger_store: StoreType::RocksDb(RocksDbConfig::new(PathBuf::from("./ledger.db"))),
            chain_store: StoreType::RocksDb(RocksDbConfig::new(PathBuf::from("./chain.db"))),
            upstream_peers: vec![],
            network: NetworkName::Preprod,
            network_magic: NetworkMagic::PREPROD,
            listen_address: "0.0.0.0:3000".to_string(),
            max_downstream_peers: 10,
            max_extra_ledger_snapshots: MaxExtraLedgerSnapshots::default(),
            migrate_chain_db: false,
            submit_api_address: None,
            ledger_vm_alloc_arena_count: 1,
            ledger_vm_alloc_arena_size: 1_024_000,
            defer_req_next_poll_ms: 200,
            trace_buffer_min_entries: 0,
            trace_buffer_max_size: 0,
            trace_dump_path: None,
            mempool: MempoolConfig::default(),
            tx_submission_responder_params: ResponderParams::default(),
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
