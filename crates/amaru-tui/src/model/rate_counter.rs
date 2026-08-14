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

use std::time::Instant;

use super::exponential_moving_average::ExponentialMovingAverage;

/// Tracks a monotonically increasing count and derives a smoothed per-second rate
/// from explicit sampling ticks.
#[derive(Debug, Clone, PartialEq)]
pub struct RateCounter {
    smoothing: usize,
    total_count: u64,
    pending_count: u64,
    last_sample_at: Option<Instant>,
    average_rate: ExponentialMovingAverage,
}

impl RateCounter {
    pub fn new(smoothing: usize) -> Self {
        Self {
            smoothing,
            total_count: 0,
            pending_count: 0,
            last_sample_at: None,
            average_rate: ExponentialMovingAverage::default(),
        }
    }

    pub fn record(&mut self, count: u64) {
        self.total_count = self.total_count.saturating_add(count);
        self.pending_count = self.pending_count.saturating_add(count);
    }

    pub fn total_count(&self) -> u64 {
        self.total_count
    }

    pub fn sample(&mut self, at: Instant) {
        let Some(last_sample_at) = self.last_sample_at else {
            self.last_sample_at = Some(at);
            self.pending_count = 0;
            return;
        };

        let elapsed = at.saturating_duration_since(last_sample_at);
        if elapsed.is_zero() {
            return;
        }

        self.average_rate.record(self.pending_count as f64 / elapsed.as_secs_f64(), self.smoothing);
        self.pending_count = 0;
        self.last_sample_at = Some(at);
    }

    pub fn rate_per_second(&self) -> f64 {
        self.average_rate.value().unwrap_or_default()
    }
}
