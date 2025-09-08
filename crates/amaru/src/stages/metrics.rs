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
