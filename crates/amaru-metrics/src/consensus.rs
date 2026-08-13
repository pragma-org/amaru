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

use std::sync::OnceLock;

use opentelemetry::{KeyValue, metrics::Meter as OpenTelemetryMeter};

use crate::{Counter, Histogram, Meter, MetricRecorder, MetricsEvent};

/// Performance measurements emitted by the select_chain stage, computed from `perf.*` events
/// but aggregatable as OpenTelemetry metrics (a duration histogram plus an outcome-labelled
/// counter per measurement kind).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ConsensusMetrics {
    /// A header reached a terminal state. The optional durations are the intervals between the
    /// virtual slot start and the tracked processing points of a header's lifecycle; they are
    /// absent when the corresponding point was never reached (e.g. a header rejected on reception
    /// carries none of them).
    HeaderLifecycle {
        outcome: String,
        /// Time from the virtual beginning of the slot to the header's reception.
        slot_start_to_header_micros: Option<u64>,
        /// Time from a header's reception to the request (or abandonment) of its block.
        block_fetch_wait_micros: Option<u64>,
        /// Time from a block's request to its reception.
        block_fetch_micros: Option<u64>,
        /// Time from a header's reception to the adoption/invalidation/abandonment of its block.
        forward_micros: Option<u64>,
    },
    /// Time from the detection of a fork to its application/abandonment.
    ForkSwitch { outcome: String, duration_micros: u64 },
}

impl MetricRecorder for ConsensusMetrics {
    fn record_to_meter(&self, meter: &Meter) {
        static HEADER_TOTAL: OnceLock<Counter<u64>> = OnceLock::new();
        static SLOT_START_TO_HEADER_DURATION: OnceLock<Histogram<u64>> = OnceLock::new();
        static HEADER_FORWARD_DURATION: OnceLock<Histogram<u64>> = OnceLock::new();
        static BLOCK_FETCH_WAIT_DURATION: OnceLock<Histogram<u64>> = OnceLock::new();
        static BLOCK_FETCH_DURATION: OnceLock<Histogram<u64>> = OnceLock::new();
        static FORK_SWITCH_DURATION: OnceLock<Histogram<u64>> = OnceLock::new();
        static FORK_SWITCH_TOTAL: OnceLock<Counter<u64>> = OnceLock::new();

        let Some(meter) = meter.get() else {
            return;
        };

        match self {
            ConsensusMetrics::HeaderLifecycle {
                outcome,
                slot_start_to_header_micros,
                block_fetch_wait_micros,
                block_fetch_micros,
                forward_micros,
            } => {
                let attributes = &[KeyValue::new("outcome", outcome.to_string())];

                // Census of every header outcome, whether or not it reached the end of the lifecycle.
                let total = HEADER_TOTAL.get_or_init(|| {
                    meter
                        .u64_counter("amaru_consensus_header_total")
                        .with_description("number of headers that reached a terminal state, labelled by outcome")
                        .build()
                });
                total.add(1, attributes);

                // Each interval is recorded only when the corresponding processing point was reached.
                record_optional_duration(
                    meter,
                    &SLOT_START_TO_HEADER_DURATION,
                    "amaru_consensus_slot_start_to_header_duration_microseconds",
                    "time from the virtual beginning of a slot to the reception of its header",
                    attributes,
                    *slot_start_to_header_micros,
                );
                record_optional_duration(
                    meter,
                    &BLOCK_FETCH_WAIT_DURATION,
                    "amaru_consensus_block_fetch_wait_duration_microseconds",
                    "time from a header's reception to its block being requested or the wait abandoned",
                    attributes,
                    *block_fetch_wait_micros,
                );
                record_optional_duration(
                    meter,
                    &BLOCK_FETCH_DURATION,
                    "amaru_consensus_block_fetch_duration_microseconds",
                    "time from a block being requested to its reception",
                    attributes,
                    *block_fetch_micros,
                );
                record_optional_duration(
                    meter,
                    &HEADER_FORWARD_DURATION,
                    "amaru_consensus_header_forward_duration_microseconds",
                    "time from a header's reception to its block being adopted, invalidated or abandoned",
                    attributes,
                    *forward_micros,
                );
            }
            ConsensusMetrics::ForkSwitch { outcome, duration_micros } => record_duration(
                meter,
                &FORK_SWITCH_DURATION,
                &FORK_SWITCH_TOTAL,
                "amaru_consensus_fork_switch_duration_microseconds",
                "time from the detection of a fork to its application or abandonment",
                "amaru_consensus_fork_switch_total",
                "number of fork switches that ended, labelled by outcome",
                outcome,
                *duration_micros,
            ),
        }
    }
}

/// Record a duration to its histogram, if present, without touching any counter.
fn record_optional_duration(
    meter: &OpenTelemetryMeter,
    duration: &'static OnceLock<Histogram<u64>>,
    duration_name: &'static str,
    duration_description: &'static str,
    attributes: &[KeyValue],
    duration_micros: Option<u64>,
) {
    let Some(duration_micros) = duration_micros else {
        return;
    };
    let duration = duration.get_or_init(|| {
        meter.u64_histogram(duration_name).with_description(duration_description).with_unit("us").build()
    });
    duration.record(duration_micros, attributes);
}

/// Record a duration measurement to its histogram and bump the matching outcome-labelled counter.
#[allow(clippy::too_many_arguments)]
fn record_duration(
    meter: &OpenTelemetryMeter,
    duration: &'static OnceLock<Histogram<u64>>,
    total: &'static OnceLock<Counter<u64>>,
    duration_name: &'static str,
    duration_description: &'static str,
    total_name: &'static str,
    total_description: &'static str,
    outcome: &str,
    duration_micros: u64,
) {
    let duration = duration.get_or_init(|| {
        meter.u64_histogram(duration_name).with_description(duration_description).with_unit("us").build()
    });
    let total = total.get_or_init(|| meter.u64_counter(total_name).with_description(total_description).build());

    let attributes = &[KeyValue::new("outcome", outcome.to_string())];
    duration.record(duration_micros, attributes);
    total.add(1, attributes);
}

impl From<ConsensusMetrics> for MetricsEvent {
    fn from(value: ConsensusMetrics) -> Self {
        MetricsEvent::ConsensusMetrics(value)
    }
}
