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

use gasket::framework::WorkerError;
pub use opentelemetry::metrics::{Counter, Gauge, Meter};

pub type MetricsPort = gasket::messaging::OutputPort<MetricsEvent>;

pub async fn send_event(port: &mut MetricsPort, event: MetricsEvent) -> Result<(), WorkerError> {
    port.send(event.into()).await.map_err(|e| {
        tracing::error!(error=%e, "failed to send event");
        gasket::framework::WorkerError::Send
    })
}

pub trait Metric: Send + Sync {
    fn record(&self, meter: &Meter);
}

#[derive(Clone)]
pub struct MetricsEvent(pub Arc<dyn Metric>);

impl Deref for MetricsEvent {
    type Target = dyn Metric;

    fn deref(&self) -> &Self::Target {
        &*self.0
    }
}
