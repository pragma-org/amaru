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

use crate::schedule;
use amaru_metrics::MetricsEvent;
use gasket::framework::{WorkSchedule, WorkerError};
use opentelemetry::metrics::{Meter, MeterProvider};
use opentelemetry_sdk::metrics::SdkMeterProvider;

pub type UpstreamPort = gasket::messaging::InputPort<MetricsEvent>;

pub struct MetricsStage {
    pub upstream: UpstreamPort,
    pub meter: Option<Meter>,
}

impl MetricsStage {
    pub fn new(maybe_provider: Option<SdkMeterProvider>) -> Self {
        // The meter is named `cardano_node_metrics` to match the metrics exported by the cardano node (https://github.com/pragma-org/amaru/issues/428)
        let meter = maybe_provider.map(|provider| provider.meter("cardano_node_metrics"));

        Self {
            upstream: Default::default(),
            meter,
        }
    }
}

impl gasket::framework::Stage for MetricsStage {
    type Unit = MetricsEvent;
    type Worker = Worker;

    fn name(&self) -> &str {
        "metrics"
    }

    fn metrics(&self) -> gasket::metrics::Registry {
        gasket::metrics::Registry::default()
    }
}

pub struct Worker;

#[async_trait::async_trait(?Send)]
impl gasket::framework::Worker<MetricsStage> for Worker {
    async fn bootstrap(_stage: &MetricsStage) -> Result<Self, WorkerError> {
        Ok(Self)
    }

    async fn schedule(
        &mut self,
        stage: &mut MetricsStage,
    ) -> Result<WorkSchedule<MetricsEvent>, WorkerError> {
        schedule!(&mut stage.upstream)
    }

    async fn execute(
        &mut self,
        unit: &MetricsEvent,
        stage: &mut MetricsStage,
    ) -> Result<(), WorkerError> {
        if let Some(meter) = &stage.meter {
            unit.record(meter);
        }

        Ok(())
    }
}
