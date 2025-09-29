// Copyright 2025 PRAGMA
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
use crate::{Counter, Gauge};

use crate::Meter;

use crate::{MetricRecorder, MetricsEvent};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LedgerMetrics {
    pub block_height: u64,
    pub txs_processed: u64,
    pub slot: u64,
    pub slot_in_epoch: u64,
    pub epoch: u64,
    pub density: f64,
}

impl Default for LedgerMetrics {
    fn default() -> Self {
        Self {
            block_height: 1,
            txs_processed: Default::default(),
            slot: Default::default(),
            slot_in_epoch: Default::default(),
            epoch: Default::default(),
            density: Default::default(),
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl MetricRecorder for LedgerMetrics {
    fn record_to_meter(&self, _meter: &Meter) {
        // no-op  in wasm32 environment, because of `opentelemetry` dependency on `js-sys`:
        // https://github.com/open-telemetry/opentelemetry-rust/blob/main/opentelemetry/src/lib.rs#L296-L301
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl MetricRecorder for LedgerMetrics {
    fn record_to_meter(&self, meter: &Meter) {
        static BLOCK_HEIGHT: OnceLock<Gauge<u64>> = OnceLock::new();
        static TXS_PROCESSED: OnceLock<Counter<u64>> = OnceLock::new();
        static SLOT_NUM: OnceLock<Gauge<u64>> = OnceLock::new();
        static SLOT_IN_EPOCH: OnceLock<Gauge<u64>> = OnceLock::new();
        static EPOCH: OnceLock<Gauge<u64>> = OnceLock::new();
        static DENSITY: OnceLock<Gauge<f64>> = OnceLock::new();

        let block_height = BLOCK_HEIGHT.get_or_init(|| {
            meter
                .u64_gauge("cardano_node_metrics_blockNum_int")
                .with_description("block height of latest processed block")
                .with_unit("int")
                .build()
        });
        let txs_processed = TXS_PROCESSED.get_or_init(|| {
            meter
                .u64_counter("cardano_node_metrics_txsProcessedNum_int")
                .with_description("total transactions processed")
                .with_unit("int")
                .build()
        });
        let slot_num = SLOT_NUM.get_or_init(|| {
            meter
                .u64_gauge("cardano_node_metrics_slotNum_int")
                .with_description("latest slot number with a block")
                .with_unit("int")
                .build()
        });
        let epoch = EPOCH.get_or_init(|| {
            meter
                .u64_gauge("cardano_node_metrics_epoch_int")
                .with_description("the epoch of the latest processed block")
                .with_unit("int")
                .build()
        });
        let slot_in_epoch = SLOT_IN_EPOCH.get_or_init(|| {
            meter
                .u64_gauge("cardano_node_metrics_slotInEpoch_int")
                .with_description("the relative slot in the epoch of the latest processed block")
                .with_unit("int")
                .build()
        });
        let density = DENSITY.get_or_init(|| {
            meter
                .f64_gauge("cardano_node_metrics_density_real")
                .with_description(
                    "chain density over the last k blocks or since genesis, whichever is shorter",
                )
                .with_unit("real")
                .build()
        });

        block_height.record(self.block_height, &[]);
        txs_processed.add(self.txs_processed, &[]);
        slot_num.record(self.slot, &[]);
        epoch.record(self.epoch, &[]);
        slot_in_epoch.record(self.slot_in_epoch, &[]);
        density.record(self.density, &[]);
    }
}

impl From<LedgerMetrics> for MetricsEvent {
    fn from(value: LedgerMetrics) -> Self {
        MetricsEvent::LedgerMetrics(value)
    }
}
