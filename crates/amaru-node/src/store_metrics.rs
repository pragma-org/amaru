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

//! RocksDB health gauges for the node's stores (the `amaru_rocksdb_*` metrics).
//!
//! The stores are sampled on a timer rather than as blocks are applied, so that the gauges keep
//! reporting while the node is not making progress, which is when they are needed most.

use std::{sync::Arc, time::Duration};

use amaru_metrics::{Meter, MetricRecorder, MetricsEvent};
use amaru_stores::rocksdb::MetricsProbe;
use tokio::{
    runtime::Handle,
    task::JoinHandle,
    time::{MissedTickBehavior, interval},
};

const SAMPLING_INTERVAL: Duration = Duration::from_secs(30);

/// The probes of the stores opened by [`crate::build_node`], handed over to the running node.
pub(crate) struct StoreMetricsProbes(pub(crate) Vec<MetricsProbe>);

/// Sample the stores into `meter`, once at startup and then every [`SAMPLING_INTERVAL`], until
/// one of them is closed.
///
/// A store is only ever held between two await points, so that aborting and joining the returned
/// task guarantees it no longer keeps any store open.
pub(crate) fn track_store_metrics(runtime: &Handle, meter: Arc<Meter>, probes: StoreMetricsProbes) -> JoinHandle<()> {
    let StoreMetricsProbes(probes) = probes;
    runtime.spawn(async move {
        let mut ticks = interval(SAMPLING_INTERVAL);
        ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            ticks.tick().await;
            for probe in &probes {
                let Some(sample) = probe.sample() else {
                    return;
                };
                MetricsEvent::from(sample).record_to_meter(&meter);
            }
        }
    })
}
