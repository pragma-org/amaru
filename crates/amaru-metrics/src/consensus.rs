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
use opentelemetry::metrics::Meter as OpenTelemetryMeter;

#[cfg(not(target_arch = "wasm32"))]
use crate::{Counter, Histogram};
use crate::{Meter, MetricRecorder, MetricsEvent};

/// Bucket boundaries, in microseconds, of the duration histograms below: 0.5 ms to 30 s.
///
/// Without them the OpenTelemetry SDK falls back to its default boundaries (0 to 10,000), which
/// are meant for milliseconds. Every duration above 10 ms then lands in the overflow bucket, so a
/// header arriving 300 ms into its slot is indistinguishable from one arriving after 20 s. The
/// 1 s, 3 s and 5 s boundaries match the block delay thresholds that cardano-node reports.
#[cfg(not(target_arch = "wasm32"))]
const DURATION_BOUNDARIES_MICROS: [f64; 16] = [
    500.0,
    1_000.0,
    2_500.0,
    5_000.0,
    10_000.0,
    25_000.0,
    50_000.0,
    100_000.0,
    250_000.0,
    500_000.0,
    1_000_000.0,
    2_000_000.0,
    3_000_000.0,
    5_000_000.0,
    10_000_000.0,
    30_000_000.0,
];

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
        /// Duration of `perf.header.block_fetch_wait`.
        block_fetch_wait_micros: Option<u64>,
        /// Duration of `perf.blocks.fetch`.
        block_fetch_micros: Option<u64>,
        /// Duration of `perf.header.forward`.
        forward_micros: Option<u64>,
    },
    /// Duration of `perf.fork.switch`.
    ForkSwitch { outcome: String, duration_micros: u64 },
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
        meter
            .u64_histogram(duration_name)
            .with_description(duration_description)
            .with_unit("us")
            .with_boundaries(DURATION_BOUNDARIES_MICROS.to_vec())
            .build()
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
        meter
            .u64_histogram(duration_name)
            .with_description(duration_description)
            .with_unit("us")
            .with_boundaries(DURATION_BOUNDARIES_MICROS.to_vec())
            .build()
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

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use opentelemetry::metrics::MeterProvider;
    use opentelemetry_sdk::metrics::{
        InMemoryMetricExporter, PeriodicReader, SdkMeterProvider,
        data::{AggregatedMetrics, MetricData},
    };

    use super::*;

    /// A header seen 300 ms into its slot must land in the (250 ms, 500 ms] bucket, not in the overflow bucket.
    #[test]
    fn header_durations_land_in_their_own_bucket() {
        let exporter = InMemoryMetricExporter::default();
        let provider =
            SdkMeterProvider::builder().with_reader(PeriodicReader::builder(exporter.clone()).build()).build();
        let meter = Meter::from(provider.meter("amaru"));

        ConsensusMetrics::HeaderLifecycle {
            outcome: "valid".to_string(),
            slot_start_to_header_micros: Some(300_000),
            block_fetch_wait_micros: None,
            block_fetch_micros: None,
            forward_micros: None,
        }
        .record_to_meter(&meter);
        provider.force_flush().expect("flush metrics");

        let exported = exporter.get_finished_metrics().expect("finished metrics");
        let histogram = exported
            .iter()
            .flat_map(|resource| resource.scope_metrics())
            .flat_map(|scope| scope.metrics())
            .find(|metric| metric.name() == "amaru_consensus_slot_start_to_header_duration_microseconds")
            .map(|metric| metric.data())
            .and_then(|data| {
                if let AggregatedMetrics::U64(MetricData::Histogram(histogram)) = data {
                    histogram.data_points().next()
                } else {
                    None
                }
            })
            .expect("slot start to header histogram");

        let bounds: Vec<f64> = histogram.bounds().collect();
        assert_eq!(bounds, DURATION_BOUNDARIES_MICROS.to_vec());

        let counts: Vec<u64> = histogram.bucket_counts().collect();
        let bucket = bounds.iter().position(|bound| *bound >= 300_000.0).expect("a bucket for 300 ms");
        assert_eq!(bounds[bucket], 500_000.0);
        assert_eq!(counts[bucket], 1);
        assert_eq!(counts.iter().sum::<u64>(), 1, "nothing else recorded, in particular not the overflow bucket");
    }
}
