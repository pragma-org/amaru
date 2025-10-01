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

use amaru_metrics::{Meter, MetricRecorder, MetricsEvent};
use pure_stage::{BoxFuture, Effects, ExternalEffect, ExternalEffectAPI, Resources, SendData};
use std::sync::Arc;

pub trait MetricsOps: Clone + Send {
    fn record(&self, event: MetricsEvent) -> BoxFuture<'static, ()>;
}

#[derive(Clone)]
pub struct Metrics<'a, T>(&'a Effects<T>);

impl<'a, T> Metrics<'a, T> {
    pub fn new(eff: &'a Effects<T>) -> Metrics<'a, T> {
        Metrics(eff)
    }
}

impl<T: Clone + SendData + Sync> MetricsOps for Metrics<'_, T> {
    fn record(&self, event: MetricsEvent) -> BoxFuture<'static, ()> {
        self.0.external(RecordMetricsEffect::new(event))
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecordMetricsEffect {
    event: MetricsEvent,
}

impl RecordMetricsEffect {
    pub fn new(event: MetricsEvent) -> Self {
        Self { event }
    }
}

pub type ResourceMeter = Arc<Meter>;

impl ExternalEffect for RecordMetricsEffect {
    fn run(self: Box<Self>, resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        Self::wrap(async move {
            if let Ok(meter) = resources.get::<ResourceMeter>() {
                self.event.record_to_meter(&meter);
            }
            // No-op if there is no meter, since metrics collecting is optional
        })
    }
}

impl ExternalEffectAPI for RecordMetricsEffect {
    type Response = ();
}
