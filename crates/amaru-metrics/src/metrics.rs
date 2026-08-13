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

use opentelemetry::metrics::Meter as OpenTelemetryMeter;
pub use opentelemetry::metrics::{Counter, Gauge, Histogram};

use crate::MetricsEvent;

#[derive(Default)]
pub struct Meter {
    open_telemetry_meter: Option<OpenTelemetryMeter>,
    local_observer: Option<Box<dyn Fn(&MetricsEvent) + Send + Sync>>,
}

impl From<OpenTelemetryMeter> for Meter {
    fn from(open_telemetry_meter: OpenTelemetryMeter) -> Self {
        Self { open_telemetry_meter: Some(open_telemetry_meter), local_observer: None }
    }
}

impl Meter {
    pub fn get(&self) -> Option<&OpenTelemetryMeter> {
        self.open_telemetry_meter.as_ref()
    }

    pub fn set_local_observer(&mut self, local_observer: Box<dyn Fn(&MetricsEvent) + Send + Sync>) {
        self.local_observer = Some(local_observer);
    }

    pub fn notify_local_observer_if_any(&self, event: &MetricsEvent) {
        if let Some(notify) = self.local_observer.as_ref() {
            notify(event);
        }
    }
}
