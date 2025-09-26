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

use std::sync::Arc;

use amaru_metrics::{Meter, MetricRecorder, MetricsEvent};
use pure_stage::{BoxFuture, ExternalEffect, ExternalEffectAPI, Resources, SendData};

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordMetricsEffect {
    event: MetricsEvent,
}

impl RecordMetricsEffect {
    pub fn new(event: MetricsEvent) -> Self {
        Self { event }
    }
}

impl ExternalEffect for RecordMetricsEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap(async move {
            if let Ok(meter) = resources.get::<Arc<Meter>>() {
                self.event.record_to_meter(&meter);
            }
            // No-op if there is no meter, since metrics collecting is optional
            // TODO: do we want to have some state so we can error
            // if there is no meter when we expect to have one?
        })
    }
}

impl ExternalEffectAPI for RecordMetricsEffect {
    type Response = ();
}
