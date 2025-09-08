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

use std::{ops::Deref, sync::Arc};

use gasket::framework::{WorkSchedule, WorkerError};
use opentelemetry::metrics::{Meter, MeterProvider};
use opentelemetry_sdk::metrics::SdkMeterProvider;

use crate::schedule;

pub trait Metric: Send + Sync {
    fn record(&self, meter: &Meter);
}

#[derive(Clone)]
pub struct MetricsEvent {
    pub metric: Arc<dyn Metric>,
}

impl Deref for MetricsEvent {
    type Target = Arc<dyn Metric>;

    fn deref(&self) -> &Self::Target {
        &self.metric
    }
}

pub type UpstreamPort = gasket::messaging::InputPort<MetricsEvent>;

pub struct MetricsStage {
    pub upstream: UpstreamPort,
    pub meter: Meter,
    pub _provider: SdkMeterProvider,
}

impl MetricsStage {
    pub fn new(provider: SdkMeterProvider) -> Self {
        let meter = provider.meter("cardano-node");

        Self {
            upstream: Default::default(),
            meter,
            _provider: provider,
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
        unit.metric.record(&stage.meter);

        Ok(())
    }
}
