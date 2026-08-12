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

use amaru_kernel::BlockHeight;
#[cfg(not(target_arch = "wasm32"))]
use opentelemetry::KeyValue;
#[cfg(not(target_arch = "wasm32"))]
use opentelemetry::metrics::Meter as OpenTelemetryMeter;

#[cfg(not(target_arch = "wasm32"))]
use crate::{Counter, Gauge, Histogram};
use crate::{Meter, MetricRecorder, MetricsEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GsmState {
    /// No chain has been adopted since startup.
    PreSyncing,
    /// The adopted chain trails the best observed block height.
    Syncing,
    /// The adopted chain has reached the best observed block height.
    CaughtUp,
}

impl GsmState {
    /// Derive Amaru's sync state from its applied and observed chain progress.
    ///
    /// `near_wall_clock_tip` is optional because core consensus currently knows only heights,
    /// while UI consumers can also compare the applied slot with the wall-clock target slot.
    pub fn from_chain_progress(
        applied_block_height: BlockHeight,
        observed_block_height: BlockHeight,
        near_wall_clock_tip: Option<bool>,
    ) -> Self {
        if applied_block_height < observed_block_height || near_wall_clock_tip == Some(false) {
            Self::Syncing
        } else {
            Self::CaughtUp
        }
    }

    pub fn is_caught_up(self) -> bool {
        self == Self::CaughtUp
    }

    fn metric_value(self) -> u64 {
        match self {
            Self::PreSyncing => 0,
            Self::Syncing => 1,
            Self::CaughtUp => 2,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
static GSM_STATE: OnceLock<Gauge<u64>> = OnceLock::new();

#[cfg(not(target_arch = "wasm32"))]
fn gsm_state(meter: &OpenTelemetryMeter) -> &'static Gauge<u64> {
    GSM_STATE.get_or_init(|| {
        // Keep the name and value mapping compatible with cardano-node:
        // https://github.com/IntersectMBO/cardano-node/blob/master/cardano-node/src/Cardano/Node/Tracing/Tracers/Consensus.hs
        meter
            .u64_gauge("cardano_node_metrics_GSM_state_int")
            .with_description("sync state: 0 = PreSyncing, 1 = Syncing, 2 = CaughtUp")
            .with_unit("int")
            .build()
    })
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn initialize_metrics(meter: &Meter) {
    let Some(meter) = meter.get() else {
        return;
    };

    gsm_state(meter).record(GsmState::PreSyncing.metric_value(), &[]);
}

/// Consensus measurements emitted by the chain-processing stages.
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
    /// Amaru's approximation of the cardano-node Genesis State Machine state.
    GsmState(GsmState),
}

#[cfg(target_arch = "wasm32")]
impl MetricRecorder for ConsensusMetrics {
    fn record_to_meter(&self, _meter: &Meter) {}
}

#[cfg(not(target_arch = "wasm32"))]
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
            ConsensusMetrics::GsmState(state) => gsm_state(meter).record(state.metric_value(), &[]),
        }
    }
}

/// Record a duration to its histogram, if present, without touching any counter.
#[cfg(not(target_arch = "wasm32"))]
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
#[cfg(not(target_arch = "wasm32"))]
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
