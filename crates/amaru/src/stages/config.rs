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

use amaru_kernel::{BlockHeader, NetworkMagic, NetworkName};
use amaru_ouroboros_traits::ChainStore;
use amaru_stores::in_memory::MemoryStore;
use amaru_stores::rocksdb::RocksDbConfig;
use anyhow::Context;
use std::fmt::Display;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

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

    // Number of allocation arenas to keep around for performing parallel evaluation of scripts in
    // the ledger.
    pub ledger_vm_alloc_arena_count: usize,

    // Initial size (in bytes) of each allocation arena to use for script evaluation in the ledger
    // virtual machine. Higher sizes means less re-allocations but more resident memory footprint
    // since the arena is leaking memory on purpose.
    pub ledger_vm_alloc_arena_size: usize,
}

impl Config {
    /// Parse the listen address into a `SocketAddr`.
    pub fn listen_address(&self) -> anyhow::Result<SocketAddr> {
        self.listen_address
            .parse()
            .context("invalid listen address")
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
            ledger_vm_alloc_arena_count: 1,
            ledger_vm_alloc_arena_size: 1_024_000,
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
                Err(e) => Err(format!(
                    "invalid max ledger snapshot, cannot parse value: {e}"
                )),
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
    use amaru_stores::rocksdb::RocksDbConfig;
    use std::path::PathBuf;

    use super::StoreType;

    #[test]
    fn test_store_path_display() {
        assert_eq!(format!("{}", StoreType::InMem(())), "<mem>");
        assert_eq!(
            format!(
                "{}",
                StoreType::<()>::RocksDb(RocksDbConfig::new(PathBuf::from("/path/to/store")))
            ),
            "RocksDbConfig { dir: /path/to/store }"
        );
        assert_eq!(
            format!(
                "{}",
                StoreType::<()>::RocksDb(RocksDbConfig::new(PathBuf::from("./relative/path")))
            ),
            "RocksDbConfig { dir: ./relative/path }"
        );
        assert_eq!(
            format!(
                "{}",
                StoreType::<()>::RocksDb(RocksDbConfig::new(PathBuf::from("")))
            ),
            "RocksDbConfig { dir:  }"
        );
    }
}
