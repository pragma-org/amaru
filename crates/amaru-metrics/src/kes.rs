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
use crate::Gauge;
use crate::{Meter, MetricRecorder, MetricsEvent};

/// The KES state of the key a block producer forges with.
///
/// cardano-node derives these from the forging hot key on every slot, so only nodes with forging
/// credentials report them, and the current period follows the wall clock rather than the chain tip.
/// The metric names are those of cardano-node, so that existing stake pool dashboards and alerts apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct KesMetrics {
    /// KES period the operational certificate was issued for.
    pub operational_certificate_start_kes_period: u64,
    /// First KES period in which the operational certificate is no longer valid.
    pub operational_certificate_expiry_kes_period: u64,
    /// KES periods left before the operational certificate expires.
    pub remaining_kes_periods: u64,
}

impl KesMetrics {
    /// The KES state at `current_kes_period` of an operational certificate issued for
    /// `start_kes_period`, which is valid for `max_kes_evolution` periods.
    pub fn new(current_kes_period: u64, start_kes_period: u64, max_kes_evolution: u64) -> Self {
        let expiry = start_kes_period.saturating_add(max_kes_evolution);
        Self {
            operational_certificate_start_kes_period: start_kes_period,
            operational_certificate_expiry_kes_period: expiry,
            remaining_kes_periods: expiry.saturating_sub(current_kes_period),
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl MetricRecorder for KesMetrics {
    fn record_to_meter(&self, _meter: &Meter) {}
}

#[cfg(not(target_arch = "wasm32"))]
impl MetricRecorder for KesMetrics {
    fn record_to_meter(&self, meter: &Meter) {
        static OPCERT_START_KES_PERIOD: OnceLock<Gauge<u64>> = OnceLock::new();
        static OPCERT_EXPIRY_KES_PERIOD: OnceLock<Gauge<u64>> = OnceLock::new();
        static REMAINING_KES_PERIODS: OnceLock<Gauge<u64>> = OnceLock::new();

        let Some(meter) = meter.get() else {
            return;
        };

        let start = OPCERT_START_KES_PERIOD.get_or_init(|| {
            meter
                .u64_gauge("cardano_node_metrics_operationalCertificateStartKESPeriod_int")
                .with_description("KES period the operational certificate was issued for")
                .with_unit("int")
                .build()
        });
        let expiry = OPCERT_EXPIRY_KES_PERIOD.get_or_init(|| {
            meter
                .u64_gauge("cardano_node_metrics_operationalCertificateExpiryKESPeriod_int")
                .with_description("KES period from which the operational certificate is no longer valid")
                .with_unit("int")
                .build()
        });
        let remaining = REMAINING_KES_PERIODS.get_or_init(|| {
            meter
                .u64_gauge("cardano_node_metrics_remainingKESPeriods_int")
                .with_description("number of remaining KES periods before the key becomes invalid")
                .with_unit("int")
                .build()
        });

        start.record(self.operational_certificate_start_kes_period, &[]);
        expiry.record(self.operational_certificate_expiry_kes_period, &[]);
        remaining.record(self.remaining_kes_periods, &[]);
    }
}

impl From<KesMetrics> for MetricsEvent {
    fn from(value: KesMetrics) -> Self {
        MetricsEvent::KesMetrics(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn exports_certificate_gauges() {
        use opentelemetry::metrics::MeterProvider;
        use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};

        let exporter = InMemoryMetricExporter::default();
        let provider =
            SdkMeterProvider::builder().with_reader(PeriodicReader::builder(exporter.clone()).build()).build();
        let meter = Meter::from(provider.meter("amaru"));
        MetricsEvent::from(KesMetrics::new(981, 976, 62)).record_to_meter(&meter);
        provider.force_flush().expect("flush metrics");

        let exported = exporter.get_finished_metrics().expect("finished metrics");
        let mut names: Vec<_> = exported
            .iter()
            .flat_map(|resource| resource.scope_metrics())
            .flat_map(|scope| scope.metrics())
            .map(|metric| metric.name())
            .collect();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "cardano_node_metrics_operationalCertificateExpiryKESPeriod_int",
                "cardano_node_metrics_operationalCertificateStartKESPeriod_int",
                "cardano_node_metrics_remainingKESPeriods_int",
            ]
        );
    }

    #[test]
    fn counts_down_from_the_operational_certificate_start() {
        // Preview: op.cert issued at period 976, maxKESEvolutions = 62.
        assert_eq!(
            KesMetrics::new(976, 976, 62),
            KesMetrics {
                operational_certificate_start_kes_period: 976,
                operational_certificate_expiry_kes_period: 1038,
                remaining_kes_periods: 62,
            }
        );
        assert_eq!(KesMetrics::new(1000, 976, 62).remaining_kes_periods, 38);
        assert_eq!(KesMetrics::new(1038, 976, 62).remaining_kes_periods, 0);
    }

    #[test]
    fn an_expired_certificate_has_no_periods_left() {
        assert_eq!(KesMetrics::new(1100, 976, 62).remaining_kes_periods, 0);
    }
}
