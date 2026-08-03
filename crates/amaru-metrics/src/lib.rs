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

use std::{
    collections::BTreeMap,
    sync::{
        Arc, LazyLock, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
};

pub use crate::{
    consensus::ConsensusMetrics,
    ledger::LedgerMetrics,
    mempool::MempoolMetrics,
    metrics::{Counter, Gauge, Histogram, Meter},
    protocol::ProtocolMetrics,
    system::SystemMetrics,
};

pub mod consensus;
pub mod ledger;
pub mod mempool;
pub mod metrics;
pub mod protocol;
pub mod system;

pub const METRICS_METER_NAME: &str = "cardano_node_metrics";

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum MetricsEvent {
    LedgerMetrics(LedgerMetrics),
    MempoolMetrics(MempoolMetrics),
    ProtocolMetrics(ProtocolMetrics),
    ConsensusMetrics(ConsensusMetrics),
    SystemMetrics(SystemMetrics),
}

pub trait MetricRecorder {
    fn record_to_meter(&self, meter: &Meter);
}

impl MetricRecorder for MetricsEvent {
    fn record_to_meter(&self, meter: &Meter) {
        match self {
            MetricsEvent::LedgerMetrics(ledger_metrics) => ledger_metrics.record_to_meter(meter),
            MetricsEvent::MempoolMetrics(mempool_metrics) => mempool_metrics.record_to_meter(meter),
            MetricsEvent::ProtocolMetrics(protocol_metrics) => protocol_metrics.record_to_meter(meter),
            MetricsEvent::ConsensusMetrics(consensus_metrics) => consensus_metrics.record_to_meter(meter),
            MetricsEvent::SystemMetrics(system_metrics) => system_metrics.record_to_meter(meter),
        }
    }
}

pub trait MetricsSubscriber: Send + Sync {
    fn record(&self, event: &MetricsEvent);
}

#[derive(Debug)]
pub struct Subscription {
    id: u64,
}

static NEXT_SUBSCRIBER_ID: AtomicU64 = AtomicU64::new(1);
static SUBSCRIBERS: LazyLock<Mutex<BTreeMap<u64, Arc<dyn MetricsSubscriber>>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

fn subscribers() -> MutexGuard<'static, BTreeMap<u64, Arc<dyn MetricsSubscriber>>> {
    SUBSCRIBERS.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn subscribe(subscriber: Arc<dyn MetricsSubscriber>) -> Subscription {
    let id = NEXT_SUBSCRIBER_ID.fetch_add(1, Ordering::Relaxed);
    subscribers().insert(id, subscriber);
    Subscription { id }
}

pub fn has_subscribers() -> bool {
    !subscribers().is_empty()
}

pub fn notify_subscribers(event: &MetricsEvent) {
    let subscribers = subscribers().values().cloned().collect::<Vec<_>>();

    for subscriber in subscribers {
        subscriber.record(event);
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        subscribers().remove(&self.id);
    }
}
