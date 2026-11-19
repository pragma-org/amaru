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

use amaru_metrics::{RocksDbMetrics, RocksDbStore};
use rocksdb::properties::{self, PropName};

/// Samples the properties RocksDB reports about a store, for as long as that store is open.
///
/// A probe never keeps its store open: it holds a weak reference, upgraded only while a sample is
/// taken. Sampling reads counters RocksDB keeps in memory, without touching the disk.
pub struct MetricsProbe(Box<dyn Fn() -> Option<RocksDbMetrics> + Send + Sync>);

impl MetricsProbe {
    pub fn new(sample: impl Fn() -> Option<RocksDbMetrics> + Send + Sync + 'static) -> Self {
        Self(Box::new(sample))
    }

    /// The current properties of the store, or `None` once the store is closed.
    pub fn sample(&self) -> Option<RocksDbMetrics> {
        (self.0)()
    }
}

/// Read the sampled properties through `property`, which returns the integer value of a RocksDB
/// property, if the database has it.
pub(crate) fn read_metrics(store: RocksDbStore, property: impl Fn(&PropName) -> Option<u64>) -> RocksDbMetrics {
    let value = |name| property(name).unwrap_or_default();
    RocksDbMetrics {
        store,
        // RocksDB has no value for levels the database does not have.
        num_files_at_level: (0..).map_while(|level| property(&properties::num_files_at_level(level))).collect(),
        num_running_compactions: value(properties::NUM_RUNNING_COMPACTIONS),
        background_errors: value(properties::BACKGROUND_ERRORS),
        block_cache_usage: value(properties::BLOCK_CACHE_USAGE),
        estimate_num_keys: value(properties::ESTIMATE_NUM_KEYS),
        live_sst_files_size: value(properties::LIVE_SST_FILES_SIZE),
        estimate_pending_compaction_bytes: value(properties::ESTIMATE_PENDING_COMPACTION_BYTES),
        is_write_stopped: value(properties::IS_WRITE_STOPPED) != 0,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use amaru_metrics::RocksDbStore;
    use tempfile::TempDir;

    use crate::rocksdb::{RocksDB, RocksDbConfig, consensus::RocksDBStore};

    #[test]
    fn samples_the_ledger_store_until_it_is_closed() {
        let dir = TempDir::new().unwrap();
        let config = RocksDbConfig::new(dir.path().to_path_buf());
        let store = RocksDB::empty(&config).unwrap();
        let probe = store.metrics_probe();

        let sample = probe.sample().expect("the store is open");
        assert_eq!(sample.store, RocksDbStore::Ledger);
        assert_eq!(sample.num_files_at_level, [0; 7]);
        assert_eq!(sample.background_errors, 0);
        assert!(!sample.is_write_stopped);

        drop(store);
        assert_eq!(probe.sample(), None);
        // The probe does not hold the database lock.
        RocksDB::empty(&config).unwrap();
    }

    #[test]
    fn samples_the_chain_store_until_it_is_closed() {
        let dir = TempDir::new().unwrap();
        let config = RocksDbConfig::new(dir.path().to_path_buf());
        let store = Arc::new(RocksDBStore::create(config.clone()).unwrap());
        let probe = store.metrics_probe();

        let sample = probe.sample().expect("the store is open");
        assert_eq!(sample.store, RocksDbStore::Chain);
        assert_eq!(sample.num_files_at_level, [0; 7]);
        // A new chain store holds its version.
        assert_eq!(sample.estimate_num_keys, 1);

        drop(store);
        assert_eq!(probe.sample(), None);
        RocksDBStore::open(&config).unwrap();
    }
}
