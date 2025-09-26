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

pub use opentelemetry::metrics::{Counter, Gauge, Meter};

use crate::ledger::LedgerMetrics;

pub mod ledger;

pub const METRICS_METER_NAME: &str = "cardano_node_metrics";

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum MetricsEvent {
    LedgerMetrics(LedgerMetrics),
}

pub trait MetricRecorder {
    fn record_to_meter(&self, meter: &opentelemetry::metrics::Meter);
}

impl MetricRecorder for MetricsEvent {
    fn record_to_meter(&self, meter: &opentelemetry::metrics::Meter) {
        match self {
            MetricsEvent::LedgerMetrics(ledger_metrics) => ledger_metrics.record_to_meter(meter),
        }
    }
}
