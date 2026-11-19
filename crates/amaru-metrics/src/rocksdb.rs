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

#[cfg(not(target_arch = "wasm32"))]
use std::sync::OnceLock;

#[cfg(not(target_arch = "wasm32"))]
use opentelemetry::KeyValue;

#[cfg(not(target_arch = "wasm32"))]
use crate::Gauge;
use crate::{Meter, MetricRecorder, MetricsEvent};

pub const NUM_FILES_AT_LEVEL: &str = "amaru_rocksdb_num_files_at_level";
pub const NUM_RUNNING_COMPACTIONS: &str = "amaru_rocksdb_num_running_compactions";
pub const BACKGROUND_ERRORS: &str = "amaru_rocksdb_background_errors";
pub const BLOCK_CACHE_USAGE: &str = "amaru_rocksdb_block_cache_usage";
pub const ESTIMATE_NUM_KEYS: &str = "amaru_rocksdb_estimate_num_keys";
pub const LIVE_SST_FILES_SIZE: &str = "amaru_rocksdb_live_sst_files_size";
pub const ESTIMATE_PENDING_COMPACTION_BYTES: &str = "amaru_rocksdb_estimate_pending_compaction_bytes";
pub const IS_WRITE_STOPPED: &str = "amaru_rocksdb_is_write_stopped";

/// The RocksDB instance a sample was taken from, reported as the `db` label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RocksDbStore {
    Ledger,
    Chain,
}

impl RocksDbStore {
    pub fn as_str(&self) -> &'static str {
        match self {
            RocksDbStore::Ledger => "ledger",
            RocksDbStore::Chain => "chain",
        }
    }
}

/// A sample of the properties RocksDB reports about one of the node's stores.
///
/// Each field is the RocksDB property of the same name, and each metric is named after it too
/// (`rocksdb.num-running-compactions` is `amaru_rocksdb_num_running_compactions`), so that the RocksDB
/// documentation applies as is.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RocksDbMetrics {
    pub store: RocksDbStore,
    /// Number of SST files per level of the LSM tree, starting at level 0.
    pub num_files_at_level: Vec<u64>,
    /// Number of compactions currently running.
    pub num_running_compactions: u64,
    /// Accumulated number of background errors.
    pub background_errors: u64,
    /// Memory used by the entries in the block cache (in bytes).
    pub block_cache_usage: u64,
    /// Estimated number of keys in the memtables and SST files.
    pub estimate_num_keys: u64,
    /// Size of the SST files of the current version (in bytes).
    pub live_sst_files_size: u64,
    /// Estimated amount of data compaction has to rewrite to bring every level under its target size (in bytes).
    pub estimate_pending_compaction_bytes: u64,
    /// Whether RocksDB has stopped accepting writes.
    pub is_write_stopped: bool,
}

#[cfg(target_arch = "wasm32")]
impl MetricRecorder for RocksDbMetrics {
    fn record_to_meter(&self, _meter: &Meter) {}
}

#[cfg(not(target_arch = "wasm32"))]
impl MetricRecorder for RocksDbMetrics {
    fn record_to_meter(&self, meter: &Meter) {
        static FILES_AT_LEVEL: OnceLock<Gauge<u64>> = OnceLock::new();
        static RUNNING_COMPACTIONS: OnceLock<Gauge<u64>> = OnceLock::new();
        static ERRORS: OnceLock<Gauge<u64>> = OnceLock::new();
        static CACHE_USAGE: OnceLock<Gauge<u64>> = OnceLock::new();
        static NUM_KEYS: OnceLock<Gauge<u64>> = OnceLock::new();
        static SST_FILES_SIZE: OnceLock<Gauge<u64>> = OnceLock::new();
        static PENDING_COMPACTION_BYTES: OnceLock<Gauge<u64>> = OnceLock::new();
        static WRITE_STOPPED: OnceLock<Gauge<u64>> = OnceLock::new();

        let Some(meter) = meter.get() else {
            return;
        };

        let files_at_level = FILES_AT_LEVEL.get_or_init(|| {
            meter
                .u64_gauge(NUM_FILES_AT_LEVEL)
                .with_description("number of SST files at each level of the LSM tree")
                .build()
        });
        let running_compactions = RUNNING_COMPACTIONS.get_or_init(|| {
            meter.u64_gauge(NUM_RUNNING_COMPACTIONS).with_description("number of compactions currently running").build()
        });
        let errors = ERRORS.get_or_init(|| {
            meter.u64_gauge(BACKGROUND_ERRORS).with_description("accumulated number of background errors").build()
        });
        let cache_usage = CACHE_USAGE.get_or_init(|| {
            meter
                .u64_gauge(BLOCK_CACHE_USAGE)
                .with_description("memory used by the entries in the block cache (in bytes)")
                .with_unit("bytes")
                .build()
        });
        let num_keys = NUM_KEYS.get_or_init(|| {
            meter
                .u64_gauge(ESTIMATE_NUM_KEYS)
                .with_description("estimated number of keys in the memtables and SST files")
                .build()
        });
        let sst_files_size = SST_FILES_SIZE.get_or_init(|| {
            meter
                .u64_gauge(LIVE_SST_FILES_SIZE)
                .with_description("size of the SST files of the current version (in bytes)")
                .with_unit("bytes")
                .build()
        });
        let pending_compaction_bytes = PENDING_COMPACTION_BYTES.get_or_init(|| {
            meter
                .u64_gauge(ESTIMATE_PENDING_COMPACTION_BYTES)
                .with_description(
                    "estimated amount of data compaction has to rewrite to bring every level under its target size (in bytes)",
                )
                .with_unit("bytes")
                .build()
        });
        let write_stopped = WRITE_STOPPED.get_or_init(|| {
            meter
                .u64_gauge(IS_WRITE_STOPPED)
                .with_description("1 when RocksDB has stopped accepting writes, 0 otherwise")
                .build()
        });

        let db = KeyValue::new("db", self.store.as_str());
        for (level, files) in self.num_files_at_level.iter().enumerate() {
            files_at_level.record(*files, &[db.clone(), KeyValue::new("level", level as i64)]);
        }
        let attributes = &[db];
        running_compactions.record(self.num_running_compactions, attributes);
        errors.record(self.background_errors, attributes);
        cache_usage.record(self.block_cache_usage, attributes);
        num_keys.record(self.estimate_num_keys, attributes);
        sst_files_size.record(self.live_sst_files_size, attributes);
        pending_compaction_bytes.record(self.estimate_pending_compaction_bytes, attributes);
        write_stopped.record(u64::from(self.is_write_stopped), attributes);
    }
}

impl From<RocksDbMetrics> for MetricsEvent {
    fn from(value: RocksDbMetrics) -> Self {
        MetricsEvent::RocksDbMetrics(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn exports_one_series_per_store_and_level() {
        use opentelemetry::metrics::MeterProvider;
        use opentelemetry_sdk::metrics::{
            InMemoryMetricExporter, PeriodicReader, SdkMeterProvider,
            data::{AggregatedMetrics, MetricData},
        };

        let exporter = InMemoryMetricExporter::default();
        let provider =
            SdkMeterProvider::builder().with_reader(PeriodicReader::builder(exporter.clone()).build()).build();
        let meter = Meter::from(provider.meter("amaru"));
        let sample = |store, num_files_at_level: &[u64]| RocksDbMetrics {
            store,
            num_files_at_level: num_files_at_level.to_vec(),
            num_running_compactions: 1,
            background_errors: 0,
            block_cache_usage: 4096,
            estimate_num_keys: 42,
            live_sst_files_size: 8192,
            estimate_pending_compaction_bytes: 0,
            is_write_stopped: true,
        };
        MetricsEvent::from(sample(RocksDbStore::Ledger, &[3, 0, 7])).record_to_meter(&meter);
        MetricsEvent::from(sample(RocksDbStore::Chain, &[1])).record_to_meter(&meter);
        provider.force_flush().expect("flush metrics");

        let exported = exporter.get_finished_metrics().expect("finished metrics");
        let mut series: Vec<_> = exported
            .iter()
            .flat_map(|resource| resource.scope_metrics())
            .flat_map(|scope| scope.metrics())
            .flat_map(|metric| {
                let AggregatedMetrics::U64(MetricData::Gauge(gauge)) = metric.data() else {
                    panic!("{} is not a u64 gauge", metric.name());
                };
                gauge.data_points().map(move |point| {
                    let mut labels: Vec<_> =
                        point.attributes().map(|attribute| format!("{}={}", attribute.key, attribute.value)).collect();
                    labels.sort_unstable();
                    format!("{}{{{}}} {}", metric.name(), labels.join(","), point.value())
                })
            })
            .collect();
        series.sort_unstable();
        assert_eq!(
            series,
            [
                "amaru_rocksdb_background_errors{db=chain} 0",
                "amaru_rocksdb_background_errors{db=ledger} 0",
                "amaru_rocksdb_block_cache_usage{db=chain} 4096",
                "amaru_rocksdb_block_cache_usage{db=ledger} 4096",
                "amaru_rocksdb_estimate_num_keys{db=chain} 42",
                "amaru_rocksdb_estimate_num_keys{db=ledger} 42",
                "amaru_rocksdb_estimate_pending_compaction_bytes{db=chain} 0",
                "amaru_rocksdb_estimate_pending_compaction_bytes{db=ledger} 0",
                "amaru_rocksdb_is_write_stopped{db=chain} 1",
                "amaru_rocksdb_is_write_stopped{db=ledger} 1",
                "amaru_rocksdb_live_sst_files_size{db=chain} 8192",
                "amaru_rocksdb_live_sst_files_size{db=ledger} 8192",
                "amaru_rocksdb_num_files_at_level{db=chain,level=0} 1",
                "amaru_rocksdb_num_files_at_level{db=ledger,level=0} 3",
                "amaru_rocksdb_num_files_at_level{db=ledger,level=1} 0",
                "amaru_rocksdb_num_files_at_level{db=ledger,level=2} 7",
                "amaru_rocksdb_num_running_compactions{db=chain} 1",
                "amaru_rocksdb_num_running_compactions{db=ledger} 1",
            ]
        );
    }
}
