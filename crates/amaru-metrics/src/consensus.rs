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
use crate::{Counter, Gauge, Histogram};
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

/// Consensus measurements. The performance ones are computed from `perf.*` events and aggregate
/// as a duration histogram plus an outcome-labelled counter per measurement kind.
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
    /// The adopted chain switched to a fork, rolling back at least one block.
    SwitchedToFork,
    /// The body of a block adopted while following the tip was received.
    BlockDelay {
        /// Time from the virtual beginning of the block's slot to the first reception of its body.
        delay_micros: u64,
        /// Delays of recent blocks, absent until enough of them have been received.
        recent: Option<BlockDelayShares>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BlockDelayShares {
    pub blocks: u64,
    pub within_1s: u64,
    pub within_3s: u64,
    pub within_5s: u64,
}

#[cfg(not(target_arch = "wasm32"))]
const LATE_BLOCK_DELAY_MICROS: u64 = 5_000_000;

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
        static FORKS: OnceLock<Counter<u64>> = OnceLock::new();
        static BLOCK_DELAY: OnceLock<Gauge<f64>> = OnceLock::new();
        static BLOCK_DELAY_WITHIN_1S: OnceLock<Gauge<f64>> = OnceLock::new();
        static BLOCK_DELAY_WITHIN_3S: OnceLock<Gauge<f64>> = OnceLock::new();
        static BLOCK_DELAY_WITHIN_5S: OnceLock<Gauge<f64>> = OnceLock::new();
        static LATE_BLOCKS: OnceLock<Counter<u64>> = OnceLock::new();

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
            ConsensusMetrics::SwitchedToFork => FORKS
                .get_or_init(|| {
                    meter
                        .u64_counter("cardano_node_metrics_forks_counter")
                        .with_description("number of times the adopted chain switched to a fork")
                        .build()
                })
                .add(1, &[]),
            ConsensusMetrics::BlockDelay { delay_micros, recent } => {
                f64_gauge(
                    meter,
                    &BLOCK_DELAY,
                    "cardano_node_metrics_blockfetchclient_blockdelay_real",
                    "delay (s) of the latest block fetch",
                )
                .record(*delay_micros as f64 / 1_000_000.0, &[]);
                if *delay_micros > LATE_BLOCK_DELAY_MICROS {
                    LATE_BLOCKS
                        .get_or_init(|| {
                            meter
                                .u64_counter("cardano_node_metrics_blockfetchclient_lateblocks_counter")
                                .with_description("number of block fetches that took longer than 5s")
                                .build()
                        })
                        .add(1, &[]);
                }
                let Some(recent) = recent.filter(|recent| recent.blocks > 0) else {
                    return;
                };
                let blocks = recent.blocks as f64;
                for (gauge, name, description, within) in [
                    (
                        &BLOCK_DELAY_WITHIN_1S,
                        "cardano_node_metrics_blockfetchclient_blockdelay_cdfOne_real",
                        "probability for block fetch to complete within 1s",
                        recent.within_1s,
                    ),
                    (
                        &BLOCK_DELAY_WITHIN_3S,
                        "cardano_node_metrics_blockfetchclient_blockdelay_cdfThree_real",
                        "probability for block fetch to complete within 3s",
                        recent.within_3s,
                    ),
                    (
                        &BLOCK_DELAY_WITHIN_5S,
                        "cardano_node_metrics_blockfetchclient_blockdelay_cdfFive_real",
                        "probability for block fetch to complete within 5s",
                        recent.within_5s,
                    ),
                ] {
                    f64_gauge(meter, gauge, name, description).record(within as f64 / blocks, &[]);
                }
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn f64_gauge<'a>(
    meter: &OpenTelemetryMeter,
    gauge: &'a OnceLock<Gauge<f64>>,
    name: &'static str,
    description: &'static str,
) -> &'a Gauge<f64> {
    gauge.get_or_init(|| meter.f64_gauge(name).with_description(description).build())
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

    /// A 300 ms duration must land in the (250 ms, 500 ms] bucket, not in the overflow bucket, for both histogram
    /// builders. Both cases share one test because the histograms live in process-wide `OnceLock`s, bound to the
    /// meter that first initialises them.
    #[test]
    fn duration_histograms_land_in_their_own_buckets() {
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
        ConsensusMetrics::ForkSwitch { outcome: "valid".to_string(), duration_micros: 300_000 }.record_to_meter(&meter);
        provider.force_flush().expect("flush metrics");

        let exported = exporter.get_finished_metrics().expect("finished metrics");
        for name in [
            "amaru_consensus_slot_start_to_header_duration_microseconds",
            "amaru_consensus_fork_switch_duration_microseconds",
        ] {
            let histogram = exported
                .iter()
                .flat_map(|resource| resource.scope_metrics())
                .flat_map(|scope| scope.metrics())
                .find(|metric| metric.name() == name)
                .map(|metric| metric.data())
                .and_then(|data| {
                    if let AggregatedMetrics::U64(MetricData::Histogram(histogram)) = data {
                        histogram.data_points().next()
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| panic!("{name} histogram"));

            let bounds: Vec<f64> = histogram.bounds().collect();
            assert_eq!(bounds, DURATION_BOUNDARIES_MICROS.to_vec(), "{name}");

            let counts: Vec<u64> = histogram.bucket_counts().collect();
            let bucket = bounds.iter().position(|bound| *bound >= 300_000.0).expect("a bucket for 300 ms");
            assert_eq!(bounds[bucket], 500_000.0, "{name}");
            assert_eq!(counts[bucket], 1, "{name}");
            assert_eq!(
                counts.iter().sum::<u64>(),
                1,
                "{name}: nothing else recorded, in particular not the overflow bucket"
            );
        }
    }
    #[test]
    fn block_delay_sets_cardano_node_gauges_and_counts_late_blocks() {
        let exporter = InMemoryMetricExporter::default();
        let provider =
            SdkMeterProvider::builder().with_reader(PeriodicReader::builder(exporter.clone()).build()).build();
        let meter = Meter::from(provider.meter("amaru"));

        ConsensusMetrics::BlockDelay {
            delay_micros: 6_500_000,
            recent: Some(BlockDelayShares { blocks: 4, within_1s: 1, within_3s: 2, within_5s: 3 }),
        }
        .record_to_meter(&meter);
        ConsensusMetrics::BlockDelay { delay_micros: 500_000, recent: None }.record_to_meter(&meter);
        provider.force_flush().expect("flush metrics");

        let exported = exporter.get_finished_metrics().expect("finished metrics");
        let metrics: Vec<_> =
            exported.iter().flat_map(|resource| resource.scope_metrics()).flat_map(|scope| scope.metrics()).collect();
        let gauge = |name: &str| {
            metrics
                .iter()
                .rev()
                .find(|metric| metric.name() == name)
                .and_then(|metric| {
                    if let AggregatedMetrics::F64(MetricData::Gauge(gauge)) = metric.data() {
                        gauge.data_points().last().map(|p| p.value())
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| panic!("{name} gauge"))
        };
        let late = metrics
            .iter()
            .rev()
            .find(|metric| metric.name() == "cardano_node_metrics_blockfetchclient_lateblocks_counter")
            .and_then(|metric| {
                if let AggregatedMetrics::U64(MetricData::Sum(sum)) = metric.data() {
                    sum.data_points().last().map(|p| p.value())
                } else {
                    None
                }
            })
            .expect("late blocks counter");

        assert_eq!(gauge("cardano_node_metrics_blockfetchclient_blockdelay_real"), 0.5);
        assert_eq!(gauge("cardano_node_metrics_blockfetchclient_blockdelay_cdfOne_real"), 0.25);
        assert_eq!(gauge("cardano_node_metrics_blockfetchclient_blockdelay_cdfThree_real"), 0.5);
        assert_eq!(gauge("cardano_node_metrics_blockfetchclient_blockdelay_cdfFive_real"), 0.75);
        assert_eq!(late, 1);
    }
}
